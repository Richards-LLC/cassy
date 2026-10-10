use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
use axum::routing::{delete, get, options, post};
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
    launches: Arc<Mutex<HashMap<std::path::PathBuf, (String, Instant, String)>>>,
    /// Outcomes of recent structured operations by (device, op_id), so a
    /// retried op_id replays its first outcome instead of running again
    /// (cas-566b). Held across the whole operation, which also serializes
    /// operations on this hub.
    operations: Arc<tokio::sync::Mutex<HashMap<(String, String), OperationReplay>>>,
    recovery: super::connection_recovery::RecoveryTelemetry,
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
            launches: Arc::new(Mutex::new(HashMap::new())),
            operations: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            recovery: super::connection_recovery::RecoveryTelemetry::default(),
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
    let recovery = (state.recovery.clone(), state.auth.clone());
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
        .route("/v1/auth/pairing/protocol", post(installation_protocol::<R>).options(preflight::<R>))
        .route(
            "/v1/auth/pairing/exchange",
            post(pairing_exchange::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/pairing/commit",
            post(installation_commit::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/pairing/abort",
            post(installation_abort::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/devices",
            get(installation_inventory::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/devices/{device}/revoke",
            post(installation_revoke::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/websocket-ticket",
            post(websocket_ticket::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/refresh",
            post(refresh_credential::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/scopes",
            post(grant_own_scopes::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/account/challenge",
            post(account_challenge::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/auth/account/enrollment",
            post(account_enrollment::<R>).options(preflight::<R>),
        )
        .route("/v1/machine", get(machine::<R>).options(preflight::<R>))
        .route(
            "/v1/launch/profiles",
            get(launch_profiles::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/diagnostics",
            get(diagnostics::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/sessions",
            get(sessions::<R>)
                .post(launch_session::<R>)
                .options(preflight::<R>),
        )
        .route("/v1/projects", get(projects::<R>).options(preflight::<R>))
        .route(
            "/v1/projects/browse",
            get(projects_browse::<R>).options(preflight::<R>),
        )
        .route("/v1/events", get(events::<R>).options(preflight::<R>))
        .route("/v1/attach", get(machine_attach::<R>))
        .route(
            "/v1/sessions/{session}",
            delete(end_session::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/sessions/{session}/operations",
            post(session_operation::<R>).options(preflight::<R>),
        )
        .route(
            "/v1/sessions/{session}/write-grants",
            post(session_write_grant::<R>).options(preflight::<R>),
        )
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
        .route(
            "/v1/sessions/{session}/artifacts/{artifact}/url",
            get(artifact_view_url::<R>).options(preflight::<R>),
        )
        .route("/{*path}", options(preflight::<R>))
        .with_state(state)
        .layer(middleware::from_fn_with_state(recovery, connection_evidence))
        .layer(middleware::from_fn_with_state(
            response_transport,
            security_headers,
        ))
}

async fn connection_evidence(
    State((telemetry, auth)): State<(super::connection_recovery::RecoveryTelemetry, Option<AuthStore>)>,
    request: Request<Body>, next: Next,
) -> Response {
    let category = super::connection_recovery::category(request.uri().path());
    let preflight = request.method() == axum::http::Method::OPTIONS;
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut response = next.run(request).await;
    let status = response.status().as_u16();
    let reason = response.headers().get("x-cas-refusal").and_then(|value| value.to_str().ok());
    if let Some(count) = telemetry.record(category, preflight, status, &request_id, reason)
        && (preflight || status == 401 || status == 403) {
        if let Some(auth) = auth {
            let _ = auth.audit_connection(category, preflight, status, &request_id, reason.map(super::connection_recovery::refusal), count);
        }
    }
    response.headers_mut().insert("x-cas-request-id", HeaderValue::from_str(&request_id).expect("UUID header"));
    // Request IDs are observable to this origin only after the route grants
    // CORS. Unbound pairing attempts must receive no CORS disclosure headers.
    if response.headers().contains_key("access-control-allow-origin") {
        let prior_expose = response.headers().get("access-control-expose-headers")
            .and_then(|value| value.to_str().ok()).unwrap_or_default();
        let expose = if prior_expose.split(',').any(|name| name.trim().eq_ignore_ascii_case("X-Cas-Request-Id")) {
            prior_expose.to_owned()
        } else if prior_expose.is_empty() {
            "X-Cas-Request-Id".to_owned()
        } else {
            format!("{prior_expose}, X-Cas-Request-Id")
        };
        response.headers_mut().insert("access-control-expose-headers", HeaderValue::from_str(&expose).expect("fixed header extension"));
    }
    response
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

/// cas-9b7d: the one reviewed cloud operator inbox API source in the hub's
/// CSP connect-src (production origin, operator path prefix only).
#[cfg(test)]
pub(crate) const OPERATOR_INBOX_CSP_SOURCE: &str = "https://petra-stella-cloud.vercel.app/api/operator/";

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
    // connect-src: `https:`/`wss:` reach the paired machines the operator
    // chose (arbitrary hub hosts) and the loopback hub. Two cloud services are
    // reviewed and named exactly (cas-9b7d): the pairing relay and, under
    // OPERATOR_INBOX_CSP_SOURCE, the production cloud operator inbox API
    // (`/api/operator/` only, no wildcard, nothing for non-production). They
    // are the embedded page's only external origins (`h4_csp_03`).
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(concat!(
            "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self' https: wss: http://127.0.0.1:* ws://127.0.0.1:* ",
            "https://petra-stella-cloud.vercel.app/api/operator/",
            "; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'; worker-src 'none'; manifest-src 'self'"
        )),
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
    if matches!(
        uri.path(),
        "/v1/auth/pairing/exchange" | "/v1/auth/pairing/commit" | "/v1/auth/pairing/abort" | "/v1/auth/pairing/protocol"
    ) {
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
    default_supervisor_cli: String,
}

async fn machine<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize(
        &state,
        HubAction::MachineRead,
        Scope::MachineRead,
        &headers,
        "GET",
        "/v1/machine",
    ) {
        return with_cors(unauthorized_for(&error), &headers);
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
            default_supervisor_cli: crate::config::Config::load(
                &crate::store::known_repos::host_cas_dir(),
            )
            .ok()
            .and_then(|config| {
                config
                    .llm
                    .map(|llm| llm.harness_for_role("supervisor").to_owned())
            })
            .unwrap_or_else(|| "claude".into()),
        })
        .into_response(),
        &headers,
    )
}

async fn launch_profiles<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize(
        &state,
        HubAction::MachineRead,
        Scope::MachineRead,
        &headers,
        "GET",
        "/v1/launch/profiles",
    ) {
        return with_cors(unauthorized_for(&error), &headers);
    }
    let (claude, codex, grok) = tokio::join!(
        tokio::task::spawn_blocking(|| super::launch_env::profiles(cas_mux::SupervisorCli::Claude)),
        tokio::task::spawn_blocking(|| super::launch_env::profiles(cas_mux::SupervisorCli::Codex)),
        tokio::task::spawn_blocking(|| {
            super::launch_env::resolve(cas_mux::SupervisorCli::Grok, None).map(|_| Vec::new())
        }),
    );
    with_cors(
        Json(serde_json::json!({
            "claude": profile_envelope(claude),
            "codex": profile_envelope(codex),
            "grok": profile_envelope(grok),
        }))
        .into_response(),
        &headers,
    )
}

#[derive(Serialize)]
struct ProfileEnvelope {
    installed: bool,
    profiles: Vec<super::launch_env::LaunchProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}

fn profile_envelope(
    result: Result<
        Result<Vec<super::launch_env::LaunchProfile>, super::launch_env::LaunchError>,
        tokio::task::JoinError,
    >,
) -> ProfileEnvelope {
    match result {
        Ok(Ok(profiles)) => ProfileEnvelope {
            installed: true,
            profiles,
            error: None,
        },
        Ok(Err(super::launch_env::LaunchError::MissingBinary { .. })) => ProfileEnvelope {
            installed: false,
            profiles: Vec::new(),
            error: None,
        },
        Ok(Err(error)) => ProfileEnvelope {
            installed: true,
            profiles: Vec::new(),
            error: Some(match error {
                super::launch_env::LaunchError::ProfileMissing { .. } => "profile_missing",
                super::launch_env::LaunchError::NotLoggedIn { .. } => "not_logged_in",
                super::launch_env::LaunchError::ProbeFailed { .. } => "cli_probe_failed",
                super::launch_env::LaunchError::MissingBinary { .. } => unreachable!(),
            }),
        },
        Err(_) => ProfileEnvelope {
            installed: true,
            profiles: Vec::new(),
            error: Some("cli_probe_failed"),
        },
    }
}

async fn diagnostics<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize(
        &state,
        HubAction::MachineRead,
        Scope::MachineRead,
        &headers,
        "GET",
        "/v1/diagnostics",
    ) {
        return with_cors(unauthorized_for(&error), &headers);
    }
    let tailscale = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::task::spawn_blocking(|| {
            super::tailscale::tailscale_command(&super::tailscale::tailscale_executable())
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
            "connection_recovery": state.recovery.snapshot(),
        }))
        .into_response(),
        &headers,
    )
}

#[derive(Serialize)]
struct SessionsResponse {
    schema_version: u32,
    /// Browser catalog expiry shares worker_status's fresh-heartbeat band.
    freshness_threshold_secs: i64,
    sessions: Vec<HubSession>,
}

#[derive(Serialize)]
struct ProjectsResponse {
    projects: Vec<super::projects::ProjectEntry>,
    browse_roots: Vec<super::projects::BrowseRoot>,
}

async fn projects<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize(
        &state,
        HubAction::MachineRead,
        Scope::MachineRead,
        &headers,
        "GET",
        "/v1/projects",
    ) {
        return with_cors(unauthorized_for(&error), &headers);
    }
    let sessions = match state.catalog.list().await {
        Ok(sessions) => sessions,
        Err(error) => return internal_error(error),
    };
    match tokio::task::spawn_blocking(move || {
        Ok::<_, anyhow::Error>(ProjectsResponse {
            projects: super::projects::list_projects(&sessions)?,
            browse_roots: super::projects::configured_launch_roots()?,
        })
    })
    .await
    {
        Ok(Ok(result)) => with_cors(Json(result).into_response(), &headers),
        Ok(Err(error)) => internal_error(error),
        Err(error) => internal_error(error.into()),
    }
}

#[derive(Deserialize)]
struct ProjectsBrowseQuery {
    root: String,
    #[serde(default)]
    path: String,
}

async fn projects_browse<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Query(query): Query<ProjectsBrowseQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize(
        &state,
        HubAction::MachineRead,
        Scope::MachineRead,
        &headers,
        "GET",
        "/v1/projects/browse",
    ) {
        return with_cors(unauthorized_for(&error), &headers);
    }
    match tokio::task::spawn_blocking(move || super::projects::browse(&query.root, &query.path))
        .await
    {
        Ok(Ok(result)) => with_cors(Json(result).into_response(), &headers),
        Ok(Err(_)) => with_cors(StatusCode::BAD_REQUEST.into_response(), &headers),
        Err(error) => internal_error(error.into()),
    }
}

async fn sessions<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Query(query): Query<SessionsQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize(
        &state,
        HubAction::SessionRead,
        Scope::SessionRead,
        &headers,
        "GET",
        "/v1/sessions",
    ) {
        return with_cors(unauthorized_for(&error), &headers);
    }
    match state.catalog.list().await {
        Ok(sessions) => with_cors(
            Json(SessionsResponse {
                schema_version: super::HUB_SCHEMA_VERSION,
                freshness_threshold_secs:
                    crate::mcp::tools::service::agent_liveness::WORKER_STALE_SECS,
                sessions: supervisor_sessions(sessions, query.workers, query.dormant),
            })
            .into_response(),
            &headers,
        ),
        Err(error) => internal_error(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchSessionRequest {
    target: super::projects::LaunchTarget,
    supervisor_cli: String,
    workers: Option<u8>,
    name: Option<String>,
    profile: Option<String>,
}

async fn launch_session<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
    Json(request): Json<LaunchSessionRequest>,
) -> Response {
    let context = match authorize(
        &state,
        HubAction::Mutation,
        Scope::SessionLaunch,
        &headers,
        "POST",
        "/v1/sessions",
    ) {
        Ok(Some(context)) => context,
        Ok(None) => return with_cors(unauthorized(), &headers),
        Err(error) if error.to_string() == "scope denied" => {
            return with_cors((StatusCode::FORBIDDEN, Json(serde_json::json!({"error":"scope_denied", "required_scope":"session:launch"}))).into_response(), &headers);
        }
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    let Some(auth) = state.auth.clone() else {
        return with_cors(unauthorized(), &headers);
    };
    let launches = state.launches.clone();
    let response = match tokio::task::spawn_blocking(move || {
        launch_session_blocking(request, &auth, &context, &launches)
    })
    .await
    {
        Ok(response) => response,
        Err(error) => launch_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "launch_failed",
            &error.to_string(),
        ),
    };
    with_cors(response, &headers)
}

/// End one factory session from Commander (cas-55a4): the operator's way to
/// retire a stale session that still looks live. It needs `factory:manage`,
/// as spawning and shutting down workers does, and stops the session's daemon
/// the way `cas kill <name>` does.
async fn end_session<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    headers: HeaderMap,
) -> Response {
    let uri = format!("/v1/sessions/{session}");
    let context = match authorize(
        &state,
        HubAction::Mutation,
        Scope::FactoryManage,
        &headers,
        "DELETE",
        &uri,
    ) {
        Ok(context) => context,
        Err(error) if error.to_string() == "scope denied" => {
            return with_cors((StatusCode::FORBIDDEN, Json(serde_json::json!({"error":"scope_denied", "required_scope":"factory:manage"}))).into_response(), &headers);
        }
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    if !valid_launch_name(&session) {
        return with_cors(generic_not_found(), &headers);
    }
    let device = context
        .as_ref()
        .map(|context| context.device_id.clone())
        .unwrap_or_else(|| "local".to_string());
    // cas-566b (brief O8): End session was the one Commander mutation with no
    // audit row. Like launch, it is refused when the requested row cannot be
    // written, and its outcome is recorded after it runs.
    let audited = state.auth.clone().zip(context.clone());
    if let Some((auth, context)) = audited.as_ref()
        && let Err(error) = auth.audit_operation(
            context,
            "requested",
            "session_end",
            Scope::FactoryManage,
            &session,
            None,
            chrono::Utc::now(),
        )
    {
        return with_cors(
            launch_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "audit_unavailable",
                &error.to_string(),
            ),
            &headers,
        );
    }
    let name = session.clone();
    let outcome =
        tokio::task::spawn_blocking(move || crate::cli::factory::end_session_by_name(&name)).await;
    if let Some((auth, context)) = audited.as_ref() {
        let (audit_outcome, detail) = match &outcome {
            Ok(Ok(crate::cli::factory::EndSessionOutcome::NotFound)) => ("not_found", None),
            Ok(Ok(crate::cli::factory::EndSessionOutcome::Ended)) => ("ended", None),
            Ok(Ok(_)) => ("cleaned_stale", None),
            Ok(Err(error)) => ("failed", Some(error.to_string())),
            Err(error) => ("failed", Some(error.to_string())),
        };
        if let Err(error) = auth.audit_operation(
            context,
            audit_outcome,
            "session_end",
            Scope::FactoryManage,
            &session,
            detail,
            chrono::Utc::now(),
        ) {
            tracing::warn!(%error, %session, "cas-566b: End session outcome audit row could not be written");
        }
    }
    let response = match outcome {
        Ok(Ok(crate::cli::factory::EndSessionOutcome::NotFound)) => generic_not_found(),
        Ok(Ok(outcome)) => {
            let ended = outcome == crate::cli::factory::EndSessionOutcome::Ended;
            tracing::info!(session = %session, device = %device, ended, "Commander ended a factory session");
            Json(serde_json::json!({
                "session": session,
                "outcome": if ended { "ended" } else { "cleaned_stale" },
            }))
            .into_response()
        }
        Ok(Err(error)) => launch_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "end_failed",
            &error.to_string(),
        ),
        Err(error) => launch_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "end_failed",
            &error.to_string(),
        ),
    };
    with_cors(response, &headers)
}

/// How long a structured operation's outcome is replayed for a retried
/// `op_id` (cas-566b): long enough to cover a dropped response and its retry.
const OPERATION_REPLAY_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
struct OperationReplay {
    at: Instant,
    status: StatusCode,
    body: serde_json::Value,
}

/// `POST /v1/sessions/{s}/operations` (fleet-operations brief, cas-566b).
#[derive(Debug, Deserialize)]
struct OperationRequest {
    op_id: String,
    /// Parsed by [`parse_fleet_operation`] so an unknown or malformed kind
    /// gets a JSON error, not the extractor's plain-text 422.
    op: serde_json::Value,
    #[serde(default)]
    expected: serde_json::Value,
}

/// Parse `op` by hand so a malformed or unknown kind gets a JSON error, not
/// the extractor's plain-text 422. Every kind the wire contract names is
/// implemented (S1-S3).
fn parse_fleet_operation(op: serde_json::Value) -> Result<FleetOperation, String> {
    serde_json::from_value::<FleetOperation>(op).map_err(|error| error.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum FleetOperation {
    /// O1: ask the session's supervisor to merge an awaiting-merge task.
    RequestMerge { task_id: String },
    /// O2: pin the session to an epic, or clear the pin.
    FocusEpic {
        #[serde(default)]
        epic_id: Option<String>,
        #[serde(default)]
        clear: bool,
    },
    /// O3: add 1-4 workers, optionally starting one on a ready task.
    SpawnWorkers {
        count: u8,
        #[serde(default)]
        task_id: Option<String>,
    },
    /// O4: pause (hold) or resume (release) a worker.
    SetWorkerHold { worker: String, hold: bool },
    /// O6: restart a worker in place; it loses its in-flight context.
    RecycleWorker { worker: String },
    /// O7: stop a worker. One worker per operation, so `expected` names it.
    ShutdownWorkers {
        workers: Vec<String>,
        #[serde(default)]
        force: bool,
    },
    /// O5: assign a task to a worker, or unassign it (null), through the
    /// supervisor's task_update (S3, cas-31f0).
    AssignTask {
        task_id: String,
        #[serde(default)]
        assignee: Option<String>,
    },
}

impl FleetOperation {
    fn action(&self) -> &'static str {
        match self {
            Self::RequestMerge { .. } => "operation:request_merge",
            Self::FocusEpic { .. } => "operation:focus_epic",
            Self::SpawnWorkers { .. } => "operation:spawn_workers",
            Self::SetWorkerHold { .. } => "operation:set_worker_hold",
            Self::RecycleWorker { .. } => "operation:recycle_worker",
            Self::ShutdownWorkers { .. } => "operation:shutdown_workers",
            Self::AssignTask { .. } => "operation:assign_task",
        }
    }

    /// O1 is an explicit supervisor message. Reversible and additive
    /// operations need `factory:operate`; destructive ones (restart, stop)
    /// need `factory:manage` (brief: distinct scopes for destructive actions).
    fn scope(&self) -> Scope {
        match self {
            Self::RequestMerge { .. } => Scope::MessageSend,
            Self::FocusEpic { .. }
            | Self::SpawnWorkers { .. }
            | Self::SetWorkerHold { .. }
            | Self::AssignTask { .. } => Scope::FactoryOperate,
            Self::RecycleWorker { .. } | Self::ShutdownWorkers { .. } => Scope::FactoryManage,
        }
    }

    fn subject(&self) -> String {
        match self {
            Self::RequestMerge { task_id } => format!("task={task_id}"),
            Self::FocusEpic { epic_id, clear } => match (epic_id, clear) {
                (_, true) => "epic=<clear>".to_string(),
                (Some(epic_id), false) => format!("epic={epic_id}"),
                (None, false) => "epic=<none>".to_string(),
            },
            Self::SpawnWorkers { count, task_id } => match task_id {
                Some(task_id) => format!("count={count} task={task_id}"),
                None => format!("count={count}"),
            },
            Self::SetWorkerHold { worker, hold } => format!("worker={worker} hold={hold}"),
            Self::RecycleWorker { worker } => format!("worker={worker}"),
            Self::ShutdownWorkers { workers, force } => {
                format!("workers={} force={force}", workers.join(","))
            }
            Self::AssignTask { task_id, assignee } => format!(
                "task={task_id} assignee={}",
                assignee.as_deref().unwrap_or("<none>")
            ),
        }
    }
}

/// One structured fleet operation from Commander (cas-566b): authorized by
/// the operation's own scope (never the terminal lease), replayed for a
/// retried `op_id`, refused as `stale` with no side effects when `expected`
/// no longer holds, audited as requested and outcome rows, and followed by a
/// `FleetChanged` event when it changed the fleet.
async fn session_operation<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    headers: HeaderMap,
    Json(request): Json<OperationRequest>,
) -> Response {
    let uri = format!("/v1/sessions/{session}/operations");
    let operation = match parse_fleet_operation(request.op) {
        Ok(operation) => operation,
        Err(detail) => {
            return with_cors(
                launch_error(StatusCode::BAD_REQUEST, "invalid_operation", &detail),
                &headers,
            );
        }
    };
    let scope = operation.scope();
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
    let op_id = request.op_id.trim().to_string();
    if op_id.is_empty() || op_id.len() > 128 {
        return with_cors(
            launch_error(
                StatusCode::BAD_REQUEST,
                "invalid_op_id",
                "op_id must be 1-128 characters",
            ),
            &headers,
        );
    }

    let mut replays = state.operations.lock().await;
    replays.retain(|_, replay| replay.at.elapsed() < OPERATION_REPLAY_TTL);
    let key = (context.device_id.clone(), op_id.clone());
    if let Some(replay) = replays.get(&key) {
        return with_cors(
            (replay.status, Json(replay.body.clone())).into_response(),
            &headers,
        );
    }

    let sessions = match state.catalog.list().await {
        Ok(sessions) => sessions,
        Err(error) => return with_cors(internal_error(error), &headers),
    };
    let Some(cas_dir) = sessions
        .iter()
        .find(|candidate| candidate.name == session)
        .and_then(|candidate| candidate.project_dir.as_deref())
        .map(|project| std::path::Path::new(project).join(".cas"))
    else {
        return with_cors(generic_not_found(), &headers);
    };

    let now = chrono::Utc::now();
    if auth.ensure_active_context(&context, now).is_err() {
        return with_cors(
            launch_error(
                StatusCode::UNAUTHORIZED,
                "revoked",
                "device credential is no longer active",
            ),
            &headers,
        );
    }
    let action = operation.action();
    let subject = operation.subject();
    if let Err(error) = auth.audit_operation(
        &context,
        "requested",
        action,
        scope,
        &session,
        Some(subject.clone()),
        now,
    ) {
        return with_cors(
            launch_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "audit_unavailable",
                &error.to_string(),
            ),
            &headers,
        );
    }

    let attribution = verified_attribution(&context);
    let operation_session = session.clone();
    let expected = request.expected;
    let outcome =
        run_fleet_operation(cas_dir, operation_session, operation, expected, attribution).await;

    use crate::ops::fleet::OperationError;
    let (status, body, audit_outcome, detail) = match outcome {
        Ok(result) => (
            StatusCode::OK,
            serde_json::json!({"op_id": op_id, "outcome": result}),
            "allowed",
            subject,
        ),
        Err(OperationError::Stale(current)) => (
            StatusCode::CONFLICT,
            serde_json::json!({"error": "stale", "current": current}),
            "stale",
            format!("{subject}; current={current}"),
        ),
        Err(OperationError::NotFound(detail)) => (
            StatusCode::NOT_FOUND,
            serde_json::json!({"error": "not_found", "detail": detail}),
            "not_found",
            format!("{subject}; {detail}"),
        ),
        Err(OperationError::Invalid(detail)) => (
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error": "invalid_operation", "detail": detail}),
            "invalid",
            format!("{subject}; {detail}"),
        ),
        Err(OperationError::Failed(detail)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": "operation_failed", "detail": detail}),
            "failed",
            format!("{subject}; {detail}"),
        ),
    };
    if let Err(error) = auth.audit_operation(
        &context,
        audit_outcome,
        action,
        scope,
        &session,
        Some(detail),
        chrono::Utc::now(),
    ) {
        tracing::warn!(%error, %session, action, "cas-566b: operation outcome audit row could not be written");
    }
    if status == StatusCode::OK {
        state.events.fleet_changed(&session);
    }
    replays.insert(
        key,
        OperationReplay {
            at: Instant::now(),
            status,
            body: body.clone(),
        },
    );
    drop(replays);
    with_cors((status, Json(body)).into_response(), &headers)
}

/// Run one operation through the shared facade, checking `expected` first.
/// Store work runs on a blocking thread; worker operations then await the
/// same `CasService` body their MCP action runs.
/// cas-ab04 (GH #1169 part 2): body of `POST /v1/sessions/{session}/write-grants`.
#[derive(Debug, serde::Deserialize)]
struct WriteGrantRequest {
    action: String,
    task: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

async fn session_write_grant<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    headers: HeaderMap,
    Json(request): Json<WriteGrantRequest>,
) -> Response {
    let uri = format!("/v1/sessions/{session}/write-grants");
    // Widening an agent's write access is a management action.
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
    let grant = match request.action.as_str() {
        "grant" => true,
        "revoke" => false,
        other => {
            return with_cors(
                launch_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_action",
                    &format!("action must be grant or revoke, not {other}"),
                ),
                &headers,
            );
        }
    };
    let sessions = match state.catalog.list().await {
        Ok(sessions) => sessions,
        Err(error) => return with_cors(internal_error(error), &headers),
    };
    let Some(cas_dir) = sessions
        .iter()
        .find(|candidate| candidate.name == session)
        .and_then(|candidate| candidate.project_dir.as_deref())
        .map(|project| std::path::Path::new(project).join(".cas"))
    else {
        return with_cors(generic_not_found(), &headers);
    };
    let now = chrono::Utc::now();
    if auth.ensure_active_context(&context, now).is_err() {
        return with_cors(
            launch_error(StatusCode::UNAUTHORIZED, "revoked", "device credential is no longer active"),
            &headers,
        );
    }
    let action = if grant { "write_grant" } else { "write_revoke" };
    let subject = format!(
        "task={} path={}",
        request.task,
        request.path.as_deref().unwrap_or("*")
    );
    if let Err(error) =
        auth.audit_operation(&context, "requested", action, scope, &session, Some(subject.clone()), now)
    {
        return with_cors(
            launch_error(StatusCode::INTERNAL_SERVER_ERROR, "audit_unavailable", &error.to_string()),
            &headers,
        );
    }

    // The hub, not the caller, writes the grant, with the authenticated
    // device as its source. Agents hold no device credential.
    let attribution = verified_attribution(&context);
    let source = crate::config::operator_policy::GrantSource::CommanderDevice {
        device_id: context.device_id.clone(),
    };
    let blocking_session = session.clone();
    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let task_store = crate::store::open_task_store(&cas_dir)?;
        let (body, receipt) = if grant {
            let recorded = crate::config::operator_policy::record_operator_grant(
                &cas_dir,
                task_store.as_ref(),
                &request.task,
                request.path.as_deref().unwrap_or_default(),
                request.mode.as_deref().unwrap_or_default(),
                request.reason.as_deref().unwrap_or_default(),
                &source,
            )?;
            let modes = recorded
                .modes
                .iter()
                .map(|mode| format!("{mode:?}").to_lowercase())
                .collect::<Vec<_>>()
                .join("+");
            let receipt = format!(
                "Operator from Commander granted write access for {}: {} ({modes}) until the task closes. Reason: {}",
                recorded.task,
                recorded.path.display(),
                recorded.reason
            );
            (serde_json::json!({"grant": recorded}), receipt)
        } else {
            let removed = crate::config::operator_policy::revoke_operator_grants(
                &cas_dir,
                task_store.as_ref(),
                &request.task,
                request.path.as_deref(),
                &source,
            )?;
            let receipt = format!(
                "Operator from Commander revoked write access for {}: {removed} grant(s) removed.",
                request.task
            );
            (serde_json::json!({"removed": removed}), receipt)
        };
        // The receipt row in the conversation: a verified operator turn.
        let summary = format!("Write access {} for {}", if grant { "granted" } else { "revoked" }, request.task);
        if let Ok(outcome) = crate::ops::fleet::enqueue_commander_message(
            &cas_dir,
            &blocking_session,
            "supervisor",
            &receipt,
            Some(&summary),
            false,
            None,
            &attribution,
        ) && matches!(outcome, cas_store::EnqueueOutcome::Created(_))
        {
            crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(&cas_dir);
        }
        Ok(body)
    })
    .await;
    let (status, body, audit_outcome) = match outcome {
        Ok(Ok(body)) => (StatusCode::OK, body, "allowed"),
        Ok(Err(error)) => (
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error": "invalid_write_grant", "detail": error.to_string()}),
            "invalid",
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": "write_grant_failed", "detail": error.to_string()}),
            "failed",
        ),
    };
    if let Err(error) = auth.audit_operation(
        &context,
        audit_outcome,
        action,
        scope,
        &session,
        Some(subject),
        chrono::Utc::now(),
    ) {
        tracing::warn!(%error, %session, action, "cas-ab04: write-grant outcome audit row could not be written");
    }
    if status == StatusCode::OK {
        state.events.fleet_changed(&session);
    }
    with_cors((status, Json(body)).into_response(), &headers)
}

async fn run_fleet_operation(
    cas_dir: std::path::PathBuf,
    session: String,
    operation: FleetOperation,
    expected: serde_json::Value,
    attribution: MessageAttribution,
) -> Result<serde_json::Value, crate::ops::fleet::OperationError> {
    use crate::ops::fleet::{self, OperationError, WorkerExpected, WorkerOperation};
    let (kind, worker, operation) = match operation {
        FleetOperation::SpawnWorkers { count, task_id } => {
            if !(1..=4).contains(&count) {
                return Err(OperationError::Invalid(
                    "count must be between 1 and 4".to_string(),
                ));
            }
            (
                "spawn_workers",
                None,
                WorkerOperation::Spawn { count, task_id },
            )
        }
        FleetOperation::SetWorkerHold { worker, hold } => (
            "set_worker_hold",
            Some(worker.clone()),
            WorkerOperation::Hold { worker, hold },
        ),
        FleetOperation::RecycleWorker { worker } => (
            "recycle_worker",
            Some(worker.clone()),
            WorkerOperation::Recycle { worker },
        ),
        FleetOperation::ShutdownWorkers { workers, force } => {
            let [worker] = <[String; 1]>::try_from(workers).map_err(|_| {
                OperationError::Invalid(
                    "shutdown_workers stops exactly one worker per operation".to_string(),
                )
            })?;
            (
                "shutdown_workers",
                Some(worker.clone()),
                WorkerOperation::Shutdown { worker, force },
            )
        }
        // O5 runs the supervisor's async task_update on the hub's runtime.
        FleetOperation::AssignTask { task_id, assignee } => {
            return run_assign_task(
                &cas_dir,
                &task_id,
                assignee.as_deref(),
                expected,
                &attribution,
            )
            .await;
        }
        other => {
            return tokio::task::spawn_blocking(move || {
                run_store_operation(&cas_dir, &session, other, expected, &attribution)
            })
            .await
            .unwrap_or_else(|error| Err(OperationError::Failed(error.to_string())));
        }
    };
    if let Some(worker) = worker {
        let expected: WorkerExpected = serde_json::from_value(expected).map_err(|error| {
            OperationError::Invalid(format!(
                "expected must name the worker and the generation you saw: {error}"
            ))
        })?;
        let (dir, name) = (cas_dir.clone(), session.clone());
        tokio::task::spawn_blocking(move || {
            fleet::check_worker_generation(&dir, &name, &worker, &expected)
        })
        .await
        .unwrap_or_else(|error| Err(OperationError::Failed(error.to_string())))?;
    }
    let detail = fleet::run_worker_operation(&cas_dir, &session, operation).await?;
    Ok(serde_json::json!({"kind": kind, "detail": detail}))
}

/// O1 and O2: operations that only touch the session's stores.
fn run_store_operation(
    cas_dir: &std::path::Path,
    session: &str,
    operation: FleetOperation,
    expected: serde_json::Value,
    attribution: &MessageAttribution,
) -> Result<serde_json::Value, crate::ops::fleet::OperationError> {
    use crate::ops::fleet::{self, FocusEpic, OperationError};
    match operation {
        FleetOperation::RequestMerge { task_id } => {
            let expected: fleet::RequestMergeExpected = serde_json::from_value(expected)
                .map_err(|error| {
                    OperationError::Invalid(format!(
                        "expected must name the task's status and tip: {error}"
                    ))
                })?;
            let notification_id =
                fleet::request_merge(cas_dir, session, &task_id, &expected, attribution)?;
            Ok(serde_json::json!({
                "kind": "request_merge",
                "task_id": task_id,
                "notification_id": notification_id,
            }))
        }
        FleetOperation::FocusEpic { epic_id, clear } => {
            #[derive(Deserialize)]
            struct Expected {
                epic_id: Option<String>,
            }
            let expected: Expected = serde_json::from_value(expected).map_err(|error| {
                OperationError::Invalid(format!(
                    "expected must name the epic the session is focused on (or null): {error}"
                ))
            })?;
            let current = fleet::pinned_epic(session);
            if current != expected.epic_id {
                return Err(OperationError::Stale(
                    serde_json::json!({"epic_id": current}),
                ));
            }
            let request = match (clear, epic_id.as_deref().map(str::trim)) {
                (true, _) => FocusEpic::Clear,
                (false, Some(epic_id)) if !epic_id.is_empty() => FocusEpic::Pin {
                    epic_id,
                    delivery_mode: None,
                },
                _ => {
                    return Err(OperationError::Invalid(
                        "focus_epic needs an epic_id or clear=true".to_string(),
                    ));
                }
            };
            let cas_root = cas_dir;
            let text = fleet::focus_epic(cas_root, session, request)?;
            let now = fleet::pinned_epic(session);
            Ok(serde_json::json!({
                "kind": "focus_epic",
                "epic_id": now,
                "prior_epic_id": current,
                "detail": text,
                // S3 (cas-31f0): Undo sends this as a new operation.
                "inverse": fleet::focus_epic_inverse(current.as_deref(), now.as_deref()),
            }))
        }
        worker => Err(OperationError::Failed(format!(
            "{} is a worker operation, not a store operation",
            worker.action()
        ))),
    }
}

/// O5 (S3, cas-31f0): the precondition names the task's `updated_at` and
/// assignee as the operator saw them.
async fn run_assign_task(
    cas_dir: &std::path::Path,
    task_id: &str,
    assignee: Option<&str>,
    expected: serde_json::Value,
    attribution: &MessageAttribution,
) -> Result<serde_json::Value, crate::ops::fleet::OperationError> {
    use crate::ops::fleet::{self, OperationError};
    let expected: fleet::AssignTaskExpected = serde_json::from_value(expected).map_err(|error| {
        OperationError::Invalid(format!(
            "expected must name the task's updated_at and current assignee (or null): {error}"
        ))
    })?;
    fleet::assign_task(cas_dir, task_id, assignee, &expected, attribution).await
}

fn launch_error(status: StatusCode, code: &str, detail: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error":code, "detail":detail})),
    )
        .into_response()
}

fn valid_launch_name(name: &str) -> bool {
    name.len() <= 80
        && name.len() >= 3
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn selected_profile<'a>(
    name: &str,
    profiles: &'a [super::launch_env::LaunchProfile],
) -> Result<&'a str, (StatusCode, &'static str)> {
    match profiles.iter().find(|row| row.name == name) {
        Some(row) if row.logged_in => Ok(&row.name),
        Some(_) => Err((StatusCode::UNPROCESSABLE_ENTITY, "not_logged_in")),
        None => Err((StatusCode::BAD_REQUEST, "invalid_profile")),
    }
}

#[cfg(test)]
mod launch_profile_tests {
    use super::*;

    #[test]
    fn only_listed_logged_in_profiles_are_selectable() {
        let rows = vec![
            super::super::launch_env::LaunchProfile {
                name: "ready".into(),
                logged_in: true,
                is_default: false,
            },
            super::super::launch_env::LaunchProfile {
                name: "signed-out".into(),
                logged_in: false,
                is_default: true,
            },
        ];
        assert_eq!(selected_profile("ready", &rows), Ok("ready"));
        assert_eq!(
            selected_profile("signed-out", &rows),
            Err((StatusCode::UNPROCESSABLE_ENTITY, "not_logged_in"))
        );
        assert_eq!(
            selected_profile("unknown", &rows),
            Err((StatusCode::BAD_REQUEST, "invalid_profile"))
        );
    }

    #[test]
    fn launch_request_accepts_profile_and_rejects_extra_fields() {
        let body = r#"{"target":{"kind":"project","id":"known"},"supervisor_cli":"claude","profile":"support@petrastella.io"}"#;
        let request: LaunchSessionRequest = serde_json::from_str(body).unwrap();
        assert_eq!(request.profile.as_deref(), Some("support@petrastella.io"));
        let extra = r#"{"target":{"kind":"project","id":"known"},"supervisor_cli":"claude","profile":"main","args":[]}"#;
        assert!(serde_json::from_str::<LaunchSessionRequest>(extra).is_err());
    }

    #[test]
    fn missing_cli_keeps_the_other_profiles_available() {
        let missing = profile_envelope(Ok(Err(
            super::super::launch_env::LaunchError::MissingBinary { cli: "codex" },
        )));
        let available = profile_envelope(Ok(Ok(vec![super::super::launch_env::LaunchProfile {
            name: "support@petrastella.io".into(),
            logged_in: true,
            is_default: true,
        }])));
        let body = serde_json::json!({"claude": available, "codex": missing});
        assert_eq!(
            body["claude"]["profiles"][0]["name"],
            "support@petrastella.io"
        );
        assert_eq!(
            body["codex"],
            serde_json::json!({"installed":false,"profiles":[]})
        );
        let failed = profile_envelope(Ok(Err(
            super::super::launch_env::LaunchError::ProbeFailed {
                cli: "codex",
                detail: "status probe timed out".into(),
            },
        )));
        assert_eq!(
            serde_json::to_value(failed).unwrap(),
            serde_json::json!({
                "installed":true,"profiles":[],"error":"cli_probe_failed"
            })
        );
    }
}

fn launch_session_blocking(
    request: LaunchSessionRequest,
    auth: &AuthStore,
    context: &AuthContext,
    launches: &Mutex<HashMap<std::path::PathBuf, (String, Instant, String)>>,
) -> Response {
    let supervisor_cli = match request.supervisor_cli.parse::<cas_mux::SupervisorCli>() {
        Ok(
            cli @ (cas_mux::SupervisorCli::Claude
            | cas_mux::SupervisorCli::Codex
            | cas_mux::SupervisorCli::Grok),
        ) => cli,
        _ => {
            return launch_error(
                StatusCode::BAD_REQUEST,
                "invalid_supervisor_cli",
                "use claude, codex, or grok",
            );
        }
    };
    let workers = request.workers.unwrap_or(0);
    if workers > 16 {
        return launch_error(
            StatusCode::BAD_REQUEST,
            "invalid_workers",
            "workers must be between 0 and 16",
        );
    }
    if request
        .name
        .as_deref()
        .is_some_and(|name| !valid_launch_name(name))
    {
        return launch_error(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "name must be 3-80 ASCII letters, digits, or hyphens and start and end with a letter or digit",
        );
    }

    let chosen_profile = if let Some(profile) = request.profile.as_deref() {
        let profiles = match super::launch_env::profiles(supervisor_cli) {
            Ok(profiles) => profiles,
            Err(error) => {
                return launch_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "cli_probe_failed",
                    &error.to_string(),
                );
            }
        };
        match selected_profile(profile, &profiles) {
            Ok(name) => name.to_owned(),
            Err((status, code)) => {
                return launch_error(status, code, "selected profile is unavailable for this CLI");
            }
        }
    } else {
        match super::launch_env::default_profile_name(supervisor_cli) {
            Ok(profile) => profile,
            Err(error) => {
                return launch_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "cli_probe_failed",
                    &error.to_string(),
                );
            }
        }
    };

    let root = match super::projects::resolve_launch_target(&request.target) {
        Ok(root) => root.path,
        Err(error) => {
            return launch_error(
                StatusCode::BAD_REQUEST,
                "invalid_target",
                &error.to_string(),
            );
        }
    };
    let mut pending = match launches.lock() {
        Ok(lock) => lock,
        Err(_) => {
            return launch_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "launch_failed",
                "launch lock poisoned",
            );
        }
    };
    // The lock covers both the live-session check and process spawn. A pending
    // name bridges the period before the daemon writes its metadata.
    let sessions = match crate::ui::factory::SessionManager::new().list_sessions() {
        Ok(sessions) => sessions,
        Err(error) => {
            return launch_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session_registry_unavailable",
                &error.to_string(),
            );
        }
    };
    if let Some(existing) = sessions.iter().find(|session| {
        session.is_running
            && session
                .metadata
                .project_dir
                .as_deref()
                .and_then(|path| std::fs::canonicalize(path).ok())
                .as_deref()
                == Some(root.as_path())
    }) {
        pending.remove(&root);
        return (
            StatusCode::OK,
            Json(serde_json::json!({"session":existing.name, "attached":true})),
        )
            .into_response();
    }
    if let Some((name, started, profile)) = pending.get(&root) {
        if started.elapsed() < Duration::from_secs(90) {
            return (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"session":name, "attached":false, "profile":profile})),
            )
                .into_response();
        }
    }
    pending.remove(&root);

    let environment = match super::launch_env::resolve(supervisor_cli, request.profile.as_deref()) {
        Ok(environment) => environment,
        Err(error) => {
            let code = match error {
                super::launch_env::LaunchError::MissingBinary { .. } => "cli_missing",
                super::launch_env::LaunchError::ProfileMissing { .. } => "profile_missing",
                super::launch_env::LaunchError::NotLoggedIn { .. } => "not_logged_in",
                super::launch_env::LaunchError::ProbeFailed { .. } => "cli_probe_failed",
            };
            return launch_error(StatusCode::UNPROCESSABLE_ENTITY, code, &error.to_string());
        }
    };
    let name = request
        .name
        .unwrap_or_else(|| crate::ui::factory::generate_session_name(None));
    if sessions.iter().any(|session| session.name == name)
        || crate::ui::factory::metadata_path(&name).exists()
    {
        return launch_error(
            StatusCode::CONFLICT,
            "name_in_use",
            "session name already exists",
        );
    }
    let now = chrono::Utc::now();
    if auth.ensure_active_context(context, now).is_err() {
        return launch_error(
            StatusCode::UNAUTHORIZED,
            "revoked",
            "device credential is no longer active",
        );
    }
    let project = root.to_string_lossy();
    if let Err(error) = auth.audit_launch(
        context,
        "requested",
        &project,
        supervisor_cli.backend().name(),
        Some(&chosen_profile),
        Some(&name),
        now,
        None,
    ) {
        return launch_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "audit_unavailable",
            &error.to_string(),
        );
    }
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            return launch_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "launch_failed",
                &error.to_string(),
            );
        }
    };
    let log_path = crate::ui::factory::daemon_log_path(&name);
    if let Some(parent) = log_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return launch_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "launch_failed",
                &error.to_string(),
            );
        }
    }
    let placement = match spawn_factory_daemon(
        &executable,
        &root,
        &name,
        workers,
        supervisor_cli,
        &environment,
        &log_path,
    ) {
        Ok(placement) => placement,
        Err(error) => {
            return launch_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "containment_unavailable",
                &error.to_string(),
            );
        }
    };
    if let Err(error) = auth.audit_launch(
        context,
        "allowed",
        &project,
        supervisor_cli.backend().name(),
        Some(&chosen_profile),
        Some(&name),
        now,
        Some(&placement),
    ) {
        tracing::warn!(session = %name, %error, "launched factory but could not write placement audit");
    }
    pending.insert(root, (name.clone(), Instant::now(), chosen_profile.clone()));
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"session":name, "attached":false, "placement":placement, "profile":chosen_profile})),
    )
        .into_response()
}

fn factory_daemon_args(
    root: &std::path::Path,
    name: &str,
    workers: u8,
    cli: cas_mux::SupervisorCli,
) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;
    vec![
        "factory".into(), "daemon".into(), "--session".into(), name.into(),
        "--cwd".into(), root.as_os_str().to_owned(), "--workers".into(),
        workers.to_string().into(), "--supervisor-cli".into(),
        OsString::from(cli.backend().name()), "--worker-cli".into(),
        OsString::from(cli.backend().name()), "--foreground".into(),
    ]
}

fn apply_launch_environment(
    command: &mut std::process::Command,
    environment: &super::launch_env::LaunchEnvironment,
) {
    for key in &environment.remove { command.env_remove(key); }
    for (key, value) in &environment.set { command.env(key, value); }
}

#[cfg(target_os = "linux")]
fn systemd_unit_command(
    executable: &std::path::Path,
    root: &std::path::Path,
    name: &str,
    workers: u8,
    cli: cas_mux::SupervisorCli,
    environment: &super::launch_env::LaunchEnvironment,
    log_path: &std::path::Path,
) -> std::process::Command {
    use std::ffi::OsString;
    let mut command = std::process::Command::new("systemd-run");
    command.arg("--user")
        .arg(format!("--unit=cas-factory-{name}"))
        .arg("--collect")
        .arg("--property=Type=exec")
        .arg(format!("--property=StandardError=append:{}", log_path.display()))
        .arg("--property=StandardOutput=null")
        .arg(format!("--working-directory={}", root.display()));
    for (key, _) in &environment.set {
        let mut option = OsString::from("--setenv=");
        option.push(key);
        command.arg(option);
    }
    for key in &environment.remove {
        let mut option = OsString::from("--property=UnsetEnvironment=");
        option.push(key);
        command.arg(option);
    }
    command.arg("--").arg(executable)
        .args(["hub", "reap-daemon", "--session", name, "--cwd"])
        .arg(root)
        .args([
            "--workers",
            &workers.to_string(),
            "--supervisor-cli",
            cli.backend().name(),
        ]);
    apply_launch_environment(&mut command, environment);
    command
}

#[cfg(target_os = "linux")]
fn cassy_scope_command(
    executable: &std::path::Path,
    root: &std::path::Path,
    args: &[std::ffi::OsString],
) -> std::process::Command {
    // Positional arguments keep project paths and names out of shell source.
    let mut command = std::process::Command::new("/bin/sh");
    command.arg("-c").arg("kill -STOP $$; exec \"$@\"").arg("sh")
        .arg(executable).args(args).current_dir(root);
    command
}

fn spawn_factory_daemon(
    executable: &std::path::Path,
    root: &std::path::Path,
    name: &str,
    workers: u8,
    cli: cas_mux::SupervisorCli,
    environment: &super::launch_env::LaunchEnvironment,
    log_path: &std::path::Path,
) -> anyhow::Result<String> {
    use std::process::{Command, Stdio};
    let args = factory_daemon_args(root, name, workers, cli);
    #[cfg(target_os = "linux")]
    {
        let output = systemd_unit_command(executable, root, name, workers, cli, environment, log_path).output();
        match output {
            Ok(output) if output.status.success() => {
                return Ok(format!("systemd:cas-factory-{name}.service"));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let unavailable = stderr.contains("Failed to connect to bus")
                    || stderr.contains("No medium found")
                    || stderr.contains("No such file or directory");
                if !unavailable {
                    anyhow::bail!("systemd-run refused factory unit: {stderr}");
                }
                tracing::warn!(session = %name, %stderr, "systemd user manager unavailable; attempting separate Cassy scope");
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(session = %name, %error, "systemd-run unavailable; attempting separate Cassy scope");
            }
            Err(error) => return Err(error.into()),
        }
        let scope =
            crate::ui::factory::cgroup::create_server_scope(name, "daemon").ok_or_else(|| {
                anyhow::anyhow!("no separate Cassy cgroup available after systemd-run failed")
            })?;
        if !crate::ui::factory::cgroup::outside_current_scope(&scope) {
            crate::ui::factory::cgroup::remove_scope(&scope);
            anyhow::bail!("separate Cassy cgroup is inside the hub's own scope");
        }
        // The fixed shell command stops itself before exec.
        let mut command = cassy_scope_command(executable, root, &args);
        command.stdin(Stdio::null()).stdout(Stdio::null());
        let log = std::fs::OpenOptions::new().create(true).append(true).open(log_path)?;
        command.stderr(Stdio::from(log));
        apply_launch_environment(&mut command, environment);
        use std::os::unix::process::CommandExt;
        unsafe { command.pre_exec(|| {
            if libc::setsid() == -1 { return Err(std::io::Error::last_os_error()); }
            Ok(())
        }); }
        let mut child = command.spawn()?;
        let pid = child.id();
        let stopped = (0..100).any(|_| {
            let state = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
            if state.lines().any(|line| line.starts_with("State:") && line.contains('T')) { return true; }
            std::thread::sleep(Duration::from_millis(10));
            false
        });
        if !stopped {
            let _ = child.kill(); let _ = child.wait();
            crate::ui::factory::cgroup::remove_scope(&scope);
            anyhow::bail!("factory launch barrier did not stop before exec");
        }
        if let Err(error) = crate::ui::factory::cgroup::add_pid(&scope, pid) {
            let _ = child.kill(); let _ = child.wait();
            crate::ui::factory::cgroup::remove_scope(&scope);
            return Err(error.into());
        }
        if unsafe { libc::kill(pid as i32, libc::SIGCONT) } != 0 {
            let error = std::io::Error::last_os_error();
            let _ = child.kill(); let _ = child.wait();
            crate::ui::factory::cgroup::remove_scope(&scope);
            return Err(error.into());
        }
        if let Some(store) = super::DaemonExitEvidenceStore::default_for_user() {
            let _ = super::supervise_spawned_daemon(name, child, store);
        }
        return Ok(format!("cassy-scope:{}", scope.display()));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut command = Command::new(executable);
        command.args(&args).current_dir(root).stdin(Stdio::null()).stdout(Stdio::null());
        let log = std::fs::OpenOptions::new().create(true).append(true).open(log_path)?;
        command.stderr(Stdio::from(log));
        apply_launch_environment(&mut command, environment);
        #[cfg(unix)] {
            use std::os::unix::process::CommandExt;
            unsafe { command.pre_exec(|| {
                if libc::setsid() == -1 { return Err(std::io::Error::last_os_error()); }
                Ok(())
            }); }
        }
        let child = command.spawn()?;
        #[cfg(unix)] if let Some(store) = super::DaemonExitEvidenceStore::default_for_user() {
            let _ = super::supervise_spawned_daemon(name, child, store);
        }
        Ok("process-session".to_string())
    }
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    #[test]
    fn daemon_arguments_keep_project_path_as_one_argument() {
        let args = factory_daemon_args(
            std::path::Path::new("/tmp/project; touch /tmp/escape"),
            "safe-session",
            2,
            cas_mux::SupervisorCli::Codex,
        );
        assert_eq!(args[5], std::ffi::OsString::from("/tmp/project; touch /tmp/escape"));
        assert_eq!(args[9], std::ffi::OsString::from("codex"));
        assert_eq!(args.len(), 13);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_unit_is_separate_and_keeps_provider_environment() {
        let environment = super::super::launch_env::LaunchEnvironment {
            executable: "/usr/bin/codex".into(),
            set: vec![("PATH".into(), "/usr/bin".into())],
            remove: vec!["INHERITED_PROFILE".into()],
        };
        let command = systemd_unit_command(
            std::path::Path::new("/usr/bin/cas"),
            std::path::Path::new("/projects/demo"), "demo-1", 0,
            cas_mux::SupervisorCli::Codex, &environment,
            std::path::Path::new("/tmp/demo.log"),
        );
        let args = command.get_args().map(|arg| arg.to_string_lossy().to_string()).collect::<Vec<_>>();
        assert!(args.contains(&"--unit=cas-factory-demo-1".to_string()));
        assert!(args.contains(&"reap-daemon".to_string()));
        assert!(!args.iter().any(|arg| arg.starts_with("--property=KillMode=")));
        assert!(args.contains(&"--setenv=PATH".to_string()));
        assert!(args.contains(&"--property=UnsetEnvironment=INHERITED_PROFILE".to_string()));
        assert!(args.contains(&"/usr/bin/cas".to_string()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cassy_scope_fallback_uses_requested_project_as_working_directory() {
        let root = std::path::Path::new("/projects/launchproj");
        let args = factory_daemon_args(root, "demo-1", 0, cas_mux::SupervisorCli::Codex);
        let command = cassy_scope_command(std::path::Path::new("/usr/bin/cas"), root, &args);
        assert_eq!(command.get_current_dir(), Some(root));
        assert!(command.get_args().any(|arg| arg == root.as_os_str()));
    }
}

async fn events<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    let context = match authorize(
        &state,
        HubAction::SessionRead,
        Scope::SessionRead,
        &headers,
        "GET",
        "/v1/events",
    ) {
        Ok(context) => context,
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    // Subscribe before snapshotting. A concurrent event can consequently be
    // replayed once and then observed live once; sequence+revision make that a
    // harmless idempotent upsert, while the ordering avoids a lost-event gap.
    let receiver = state.events.subscribe();
    let history = state.events.history();
    let metadata = Event::default().event("stream_metadata").json_data(serde_json::json!({
        "kind": "stream_metadata", "epoch": state.events.epoch.as_str(),
        "oldest_sequence": history.first().map_or(0, |event| event.sequence),
        "latest_sequence": history.last().map_or(0, |event| event.sequence),
        "retained": history.len(),
    })).expect("fixed stream metadata");
    let initial = stream::iter(vec![Ok::<Event, Infallible>(metadata)]);
    let replay = stream::iter(
        history
            .into_iter()
            .map(|event| Ok::<Event, Infallible>(machine_event_sse(event))),
    );
    let audit = state.auth.clone();
    let live_context = context.clone();
    let live = stream::unfold((receiver, false), move |(mut receiver, ended)| {
        let audit = audit.clone();
        let context = live_context.clone();
        async move {
        if ended { return None; }
            match receiver.recv().await {
                Ok(event) => {
                    Some((Ok::<Event, Infallible>(machine_event_sse(event)), (receiver, false)))
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    let request_id = context.as_ref().map(|value| value.request_id.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    if let Some(audit) = audit {
                        let _ = audit.audit_connection("events", false, 200, &request_id, Some("viewer_lagged"), skipped);
                    }
                    let event = Event::default().event("viewer_lagged").json_data(serde_json::json!({
                        "kind": "viewer_lagged", "skipped": skipped, "request_id": request_id,
                    })).expect("fixed lag schema");
                    // End after the explicit marker. Reconnecting snapshots
                    // retained revisions; silently skipping would lose them.
                    Some((Ok::<Event, Infallible>(event), (receiver, true)))
                },
                Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
            }
        }
    });
    let auth = state.auth.clone();
    let mut ticks = tokio::time::interval(Duration::from_millis(250));
    let termination = async move {
        loop {
            // A newly-created timer may yield even on its first due tick.
            // Check the grant before that yield: buffered metadata/replay
            // must not escape before the live tail gets polled (cas-2b3a5).
            if let (Some(auth), Some(context)) = (&auth, &context) {
                if auth
                    .ensure_active_context(context, chrono::Utc::now())
                    .is_err()
                {
                    break;
                }
            }
            ticks.tick().await;
        }
    };
    let complete = stream::iter(vec![Ok::<Event, Infallible>(Event::default().event("replay_complete")
        .json_data(serde_json::json!({"kind":"replay_complete"})).expect("fixed replay marker"))]);
    let output = initial.chain(replay).chain(complete).chain(live).take_until(termination);
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
    if let Err(error) = authorize(
        &state,
        HubAction::SessionRead,
        Scope::SessionRead,
        &headers,
        "GET",
        &uri,
    ) {
        return with_cors(unauthorized_for(&error), &headers);
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

/// A short-lived signed URL for viewing an artifact the session published
/// (cassy#910), so a Commander report card can open the hosted copy. The
/// machine asks Cloud with its own credentials: the browser never holds a
/// Cloud token, and the URL it gets points at the blob store's own origin.
/// Only a record Cloud committed has one; the reply says why otherwise.
async fn artifact_view_url<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path((session, artifact)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let uri = format!("/v1/sessions/{session}/artifacts/{artifact}/url");
    if let Err(error) = authorize(
        &state,
        HubAction::SessionRead,
        Scope::SessionRead,
        &headers,
        "GET",
        &uri,
    ) {
        // With CORS, so Commander reads a refused pairing as a refusal, not as
        // an unreachable machine (cas-e503).
        return with_cors(unauthorized_for(&error), &headers);
    }
    let outcome = tokio::task::spawn_blocking(move || {
        let session = crate::bridge::server::session::resolve_session_by_name(&session)?;
        let root =
            crate::bridge::server::session::cas_root_for_session_with_fallback(&session, None)?;
        let store = cas_store::SqliteArtifactStore::open(&root)?;
        let client = crate::artifacts::cloud_client(&root);
        Ok::<_, anyhow::Error>(crate::artifacts::signed_view(
            &store,
            client.as_ref(),
            &artifact,
        ))
    })
    .await;
    // Every answer carries CORS, so Commander reads a missing session or an
    // internal error as an answer rather than as an unreachable machine
    // (cas-e503).
    let response = match outcome {
        Err(error) => internal_error(error.into()),
        Ok(Err(_)) => generic_not_found(),
        Ok(Ok(Ok(signed))) => Json(serde_json::json!({
            "artifact_id": signed.artifact.id,
            "cloud_artifact_id": signed.view.artifact_id,
            "url": signed.view.url,
            "expires_at": signed.view.expires_at,
            "name": signed.view.name.unwrap_or(signed.artifact.name),
            "mime": signed.view.mime.unwrap_or(signed.artifact.mime),
            "size_bytes": signed.view.size_bytes.unwrap_or(signed.artifact.size_bytes),
        }))
        .into_response(),
        Ok(Ok(Err(error))) => artifact_view_error(error),
    };
    let mut response = with_cors(response, &headers);
    // The URL is a short-lived capability: never cache the answer.
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

/// The reply for an artifact that has no view URL, with a stable `error`
/// code Commander turns into plain words.
fn artifact_view_error(error: crate::artifacts::ViewError) -> Response {
    use crate::artifacts::ViewError;
    use crate::artifacts::cloud::ViewFailure;
    let (status, code, detail) = match &error {
        ViewError::Unknown(_) => return generic_not_found(),
        ViewError::NotInCloud { status } => (
            StatusCode::CONFLICT,
            "artifact_not_in_cloud",
            Some(status.clone()),
        ),
        ViewError::NotLoggedIn => (
            StatusCode::SERVICE_UNAVAILABLE,
            "cloud_not_configured",
            None,
        ),
        ViewError::Cloud(ViewFailure::NotCommitted { status, .. }) => (
            StatusCode::CONFLICT,
            "artifact_not_committed",
            status.clone(),
        ),
        ViewError::Cloud(ViewFailure::NotFound { .. }) => {
            (StatusCode::NOT_FOUND, "cloud_artifact_not_found", None)
        }
        ViewError::Cloud(ViewFailure::NotLive { .. }) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "cloud_storage_not_live",
            None,
        ),
        ViewError::Cloud(ViewFailure::Failed { .. }) => {
            (StatusCode::BAD_GATEWAY, "cloud_failed", None)
        }
        ViewError::Store(_) => {
            return internal_error(anyhow::anyhow!("{error}"));
        }
    };
    // The failing interaction goes to the machine's log, not to the browser.
    tracing::warn!(%error, code, "artifact view URL unavailable");
    (
        status,
        Json(serde_json::json!({ "error": code, "status": detail })),
    )
        .into_response()
}

#[derive(Debug, Default, Deserialize)]
struct AttachQuery {
    #[serde(default)]
    panes: String,
    #[serde(default)]
    ticket: String,
    /// Off by default: worker panes are hidden and never streamed unless the
    /// viewer asks with `workers=1` (cas-6261).
    #[serde(default, deserialize_with = "flag")]
    workers: bool,
}

/// Accepts `1`, `true`, `yes`, or `on` as an enabled query flag.
fn flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let raw = String::deserialize(deserializer)?;
    Ok(matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    ))
}

#[derive(Debug, Deserialize, Default)]
struct SessionsQuery {
    #[serde(default, deserialize_with = "flag")]
    workers: bool,
    /// Dormant sessions are hidden by default, but remain discoverable for
    /// recovery when the Commander explicitly asks to show them.
    #[serde(default, deserialize_with = "flag")]
    dormant: bool,
}

/// The default catalog lists every fresh supervisor-led session, including
/// sessions whose live supervisor has not spawned workers yet. The two
/// visibility controls are independent: `workers=1` includes live worker-only
/// rows, while `dormant=1` includes sessions whose supervisor is not live.
pub(crate) fn supervisor_sessions(
    sessions: Vec<HubSession>,
    reveal_workers: bool,
    reveal_dormant: bool,
) -> Vec<HubSession> {
    if reveal_workers && reveal_dormant {
        return sessions;
    }
    sessions
        .into_iter()
        .filter(|session| {
            reveal_dormant || (!session.dormant && session.liveness == super::DaemonLiveness::Live)
        })
        .filter(|session| reveal_workers || !session.supervisor.trim().is_empty())
        .collect()
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
                query.workers,
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
    reveal_workers: bool,
) {
    let Ok(mut viewer) = connector
        .attach(&session, port, panes, daemon_identity)
        .await
    else {
        return;
    };
    let mut worker_gate = super::WorkerGate::new(reveal_workers);
    let (mut sink, mut source) = socket.split();
    // MessageQueued and ConversationHistory have no socket field: retain the
    // authenticated submitter's correlation id so only that socket receives
    // its private response.
    let mut pending_message_refs = HashSet::<(String, String)>::new();
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
                    let Some(frame) = worker_gate.admit(frame) else { continue };
                    if !operator_reply_allowed(&auth, &frame.bytes) {
                        continue;
                    }
                    if !operator_message_allowed(&auth, &frame.bytes) {
                        continue;
                    }
                    if !correlated_daemon_frame_allowed(
                        &mut pending_message_refs,
                        &session,
                        &frame.bytes,
                    ) {
                        continue;
                    }
                    trace_conversation_history_relay(&session, &frame.bytes);
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
                    let client_ref = client_correlation_ref(text.as_bytes());
                    if let Some(client_ref) = client_ref.as_deref() {
                        pending_message_refs.insert((session.clone(), client_ref.to_owned()));
                    }
                    if let Err(failure) = handle_client_message(&connector, &session, &auth, text.as_bytes()).await {
                        if let Some(client_ref) = client_ref.as_deref() {
                            pending_message_refs.remove(&(session.clone(), client_ref.to_owned()));
                        }
                        let error = legacy_send_error(&failure, client_ref.as_deref());
                        let _ = sink.send(Message::Text(error.to_string().into())).await;
                        // cas-0653: this socket's daemon upstream is gone.
                        // Closing it (not with 1000) makes the browser attach
                        // again, which restarts the upstream, and the held
                        // send goes out on that live attach.
                        if is_upstream_unavailable(&failure) {
                            let _ = sink.send(Message::Close(None)).await;
                            break;
                        }
                    }
                }
                Some(Ok(Message::Binary(bytes))) => {
                    let client_ref = client_correlation_ref(&bytes);
                    if let Some(client_ref) = client_ref.as_deref() {
                        pending_message_refs.insert((session.clone(), client_ref.to_owned()));
                    }
                    if let Err(failure) = handle_client_message(&connector, &session, &auth, &bytes).await {
                        if let Some(client_ref) = client_ref.as_deref() {
                            pending_message_refs.remove(&(session.clone(), client_ref.to_owned()));
                        }
                        let error = legacy_send_error(&failure, client_ref.as_deref());
                        let _ = sink.send(Message::Text(error.to_string().into())).await;
                        // cas-0653: this socket's daemon upstream is gone.
                        // Closing it (not with 1000) makes the browser attach
                        // again, which restarts the upstream, and the held
                        // send goes out on that live attach.
                        if is_upstream_unavailable(&failure) {
                            let _ = sink.send(Message::Close(None)).await;
                            break;
                        }
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
    store.ensure_active_context(context, now)?;
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
        // cas-e8df: identity comes from the authenticated device session, never
        // from what the client put in the frame. Whatever labels arrived are
        // discarded here, and `operator_verified` is only ever set on this path.
        // Everything else on the frame — `client_ref`, `in_reply_to` (the ask
        // this message answers, cas-a8ea8) — is forwarded unchanged.
        *attribution = verified_attribution(context);
    }
    if let ClientMessage::ConversationHistoryRequest { device_id, .. }
        | ClientMessage::OperatorReplyPersisted { device_id, .. } = &mut message {
        // History is private to the authenticated paired device. Do not trust
        // a browser-supplied selector, even though this is a read operation.
        *device_id = context.device_id.clone();
    }
    if let ClientMessage::ConversationHistoryRequest {
        request_id,
        before,
        limit,
        device_id,
    } = &message
    {
        tracing::info!(
            session,
            %request_id,
            ?before,
            limit,
            device = %device_id,
            "forwarding Commander conversation history request"
        );
    }
    if let Err(error) = connector.send(session, message).await {
        // cas-a0e2: the hub accepted the device and the message, but the
        // session's daemon upstream is not there to take it. Record that,
        // so a "Not sent" has an audit row, and hand the error back
        // unchanged for the caller to answer `upstream_unavailable`.
        if let Err(audit_error) = store.audit(
            Some(context),
            "unavailable",
            if read_message {
                "websocket_read"
            } else {
                "websocket_mutation"
            },
            Some(scope),
            Some(session),
            now,
        ) {
            tracing::warn!(session, error = %audit_error, "cas-a0e2: upstream-unavailable audit not written");
        }
        return Err(error);
    }
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

/// The attribution a hub-authenticated Commander send carries downstream
/// (cas-e8df): every field from the device session's credential record.
pub(crate) fn verified_attribution(context: &AuthContext) -> MessageAttribution {
    MessageAttribution {
        device_id: Some(context.device_id.clone()),
        credential_id: Some(context.credential_id.clone()),
        device_label: Some(context.device_label.clone()),
        operator_label: Some(context.operator_label.clone()),
        controller_origin: Some(context.controller_origin.clone()),
        request_id: Some(context.request_id.clone()),
        scopes: context
            .scopes
            .iter()
            .map(|scope| scope.as_str().to_owned())
            .collect(),
        operator_verified: true,
    }
}

pub(crate) fn is_pane_read_message(message: &ClientMessage) -> bool {
    matches!(
        message,
        ClientMessage::RequestPaneKeyframe { .. }
            | ClientMessage::ScrollbackRequest { .. }
            | ClientMessage::ConversationHistoryRequest { .. }
            | ClientMessage::OperatorReplyPersisted { .. }
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
        Err(PairingExchangeError::Conflict) if bound_origin => with_cors((StatusCode::CONFLICT, Json(serde_json::json!({"error":"installation_conflict"}))).into_response(), &headers),
        Err(PairingExchangeError::Throttled {
            retry_after_seconds,
        }) if bound_origin => with_cors(pairing_throttled(retry_after_seconds), &headers),
        Err(_) if bound_origin => with_cors(unauthorized(), &headers),
        Err(_) => unauthorized(),
    }
}

#[derive(Deserialize)]
struct InstallationProtocolRequest { controller_origin: String, pairing_token_hash: String }

async fn installation_protocol<R: SessionReadModel>(
    State(state): State<HubState<R>>, headers: HeaderMap, Json(request): Json<InstallationProtocolRequest>,
) -> Response {
    if origin(&headers).as_deref() != Some(request.controller_origin.as_str()) {
        return unauthorized();
    }
    if state.auth.as_ref().is_none_or(|auth| !auth.installation_protocol_matches(&request.pairing_token_hash, &request.controller_origin, chrono::Utc::now()).unwrap_or(false)) {
        return unauthorized();
    }
    with_cors(Json(serde_json::json!({"installation_protocol":1})).into_response(), &headers)
}

async fn installation_commit<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
    Json(action): Json<super::auth::InstallationAction>,
) -> Response {
    installation_transition(state, headers, action, true)
}
async fn installation_abort<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
    Json(action): Json<super::auth::InstallationAction>,
) -> Response {
    installation_transition(state, headers, action, false)
}
fn installation_transition<R: SessionReadModel>(
    state: HubState<R>,
    headers: HeaderMap,
    action: super::auth::InstallationAction,
    commit: bool,
) -> Response {
    if origin(&headers).as_deref() != Some(action.controller_origin.as_str()) {
        return unauthorized();
    }
    let Some(auth) = state.auth else {
        return unauthorized();
    };
    match auth.installation_action(action, commit, chrono::Utc::now()) {
        Ok(()) => with_cors(StatusCode::NO_CONTENT.into_response(), &headers),
        Err(_) => with_cors(
            (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error":"installation_conflict"})),
            )
                .into_response(),
            &headers,
        ),
    }
}
async fn installation_inventory<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    let context = match authorize(
        &state,
        HubAction::MachineRead,
        Scope::MachineRead,
        &headers,
        "GET",
        "/v1/auth/devices",
    ) {
        Ok(Some(context)) => context,
        _ => return with_cors(unauthorized(), &headers),
    };
    let Some(auth) = &state.auth else {
        return unauthorized();
    };
    match auth.list_devices() {
        Ok(devices) => with_cors(
            Json(
                devices
                    .into_iter()
                    .filter(|d| context.has(Scope::HubAdmin) || d.device_id == context.device_id)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
            &headers,
        ),
        Err(error) => with_cors(internal_error(error), &headers),
    }
}
async fn installation_revoke<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(device): Path<String>,
    headers: HeaderMap,
) -> Response {
    let uri = format!("/v1/auth/devices/{device}/revoke");
    let context = match authorize(
        &state,
        HubAction::Mutation,
        Scope::MachineRead,
        &headers,
        "POST",
        &uri,
    ) {
        Ok(Some(context)) => context,
        _ => return with_cors(unauthorized(), &headers),
    };
    if context.device_id != device && !context.has(Scope::HubAdmin) {
        return with_cors(unauthorized(), &headers);
    }
    let Some(auth) = &state.auth else {
        return unauthorized();
    };
    if auth
        .ensure_active_context(&context, chrono::Utc::now())
        .is_err()
    {
        return with_cors(unauthorized(), &headers);
    }
    match auth.revoke_installation(&context, &device, chrono::Utc::now()) {
        Ok(_) => with_cors(StatusCode::NO_CONTENT.into_response(), &headers),
        Err(error) => with_cors(internal_error(error), &headers),
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
        Err(error) => with_cors(unauthorized_for(&error), &headers),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelfGrantRequest {
    add: Vec<String>,
}

async fn grant_own_scopes<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
    Json(request): Json<SelfGrantRequest>,
) -> Response {
    let Some(auth) = &state.auth else {
        return with_cors(unauthorized(), &headers);
    };
    let context = (|| -> anyhow::Result<AuthContext> {
        let origin = request_origin(&state, HubAction::Mutation, &headers, "POST")?;
        let authorization = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .context("authorization required")?;
        let proof = headers
            .get("dpop")
            .and_then(|value| value.to_str().ok())
            .context("proof required")?;
        auth.authenticate_dpop(
            authorization,
            proof,
            &origin,
            "POST",
            "/v1/auth/scopes",
            chrono::Utc::now(),
        )
    })();
    let context = match context {
        Ok(context) => context,
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    // cas-9b08: session launch and factory:operate are the only scopes a
    // device may add itself; factory:manage and hub:admin need an invitation.
    let scope = match request.add.as_slice() {
        [one] if one == "session-launch" => Scope::SessionLaunch,
        [one] if one == "factory-operate" => Scope::FactoryOperate,
        _ => {
            return with_cors((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"invalid_scope", "detail":"Only session-launch or factory-operate may be added"}))).into_response(), &headers);
        }
    };
    match auth.grant_own_scope(&context, scope, chrono::Utc::now()) {
        Ok(scopes) => with_cors(Json(serde_json::json!({"scopes":scopes})).into_response(), &headers),
        Err(error) if error.to_string() == "scope denied" => with_cors((StatusCode::FORBIDDEN, Json(serde_json::json!({"error":"scope_denied", "detail": if scope == Scope::FactoryOperate { "Pair with a control invitation to allow managing workers" } else { "Pair with a control invitation to allow starting sessions" }}))).into_response(), &headers),
        Err(error) => {
            tracing::error!(%error, scope = scope.as_str(), "self-grant failed");
            with_cors((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"grant_failed"}))).into_response(), &headers)
        }
    }
}

/// DPoP-authenticate a mutation on `path` (the grant_own_scopes pattern).
fn authenticate_mutation<R: SessionReadModel>(
    state: &HubState<R>,
    auth: &AuthStore,
    headers: &HeaderMap,
    path: &str,
) -> anyhow::Result<AuthContext> {
    let origin = request_origin(state, HubAction::Mutation, headers, "POST")?;
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .context("authorization required")?;
    let proof = headers
        .get("dpop")
        .and_then(|value| value.to_str().ok())
        .context("proof required")?;
    auth.authenticate_dpop(authorization, proof, &origin, "POST", path, chrono::Utc::now())
}

/// cas-4634: a one-use challenge for this device's account enrollment
/// assertion (contract §5.5). The device names it to the cloud, which signs
/// it into a `psc-op-enrollment+jwt` for this hub.
async fn account_challenge<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    let Some(auth) = &state.auth else {
        return with_cors(unauthorized(), &headers);
    };
    let context = match authenticate_mutation(&state, auth, &headers, "/v1/auth/account/challenge") {
        Ok(context) => context,
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    match auth.issue_account_challenge(&context, chrono::Utc::now()) {
        Ok((challenge, expires_at)) => with_cors(
            Json(serde_json::json!({"hub_id": state.machine.id, "hub_challenge": challenge, "expires_at": expires_at})).into_response(),
            &headers,
        ),
        Err(error) => {
            tracing::error!(%error, "account challenge failed");
            with_cors((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"challenge_failed"}))).into_response(), &headers)
        }
    }
}

#[derive(Deserialize)]
struct AccountEnrollmentRequest {
    assertion: String,
}

/// cas-4634: verify the cloud's enrollment assertion and bind this exact
/// installation to the asserted account device. Every refusal is a closed
/// code; the assertion itself is never logged.
async fn account_enrollment<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
    Json(request): Json<AccountEnrollmentRequest>,
) -> Response {
    let Some(auth) = state.auth.clone() else {
        return with_cors(unauthorized(), &headers);
    };
    let context = match authenticate_mutation(&state, &auth, &headers, "/v1/auth/account/enrollment") {
        Ok(context) => context,
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
    };
    if request.assertion.len() > 16 * 1024 {
        return with_cors((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"assertion_malformed"}))).into_response(), &headers);
    }
    let verifier = crate::hub::operator_inbox::assertion::HubVerifier::shared(auth.state_dir());
    let token = request.assertion;
    let outcome = tokio::task::spawn_blocking(move || {
        let now = chrono::Utc::now();
        let (assertion, account) = match verifier.verify(&token, now) {
            Ok(verified) => verified,
            Err(error) => return Ok(Err(error.code())),
        };
        auth.bind_account(&context, &assertion, Some(&account), now)
            .map(|bound| bound.map_err(|refusal| match refusal {
                crate::hub::auth::EnrollmentRefusal::ChallengeUnknown => "challenge_unknown",
                crate::hub::auth::EnrollmentRefusal::ChallengeExpired => "challenge_expired",
                crate::hub::auth::EnrollmentRefusal::WrongHub => "wrong_hub",
                crate::hub::auth::EnrollmentRefusal::AssertionExpired => "assertion_expired",
                crate::hub::auth::EnrollmentRefusal::InstallationMismatch => "installation_mismatch",
                crate::hub::auth::EnrollmentRefusal::OriginMismatch => "origin_mismatch",
                crate::hub::auth::EnrollmentRefusal::AccountMismatch => "account_mismatch",
                crate::hub::auth::EnrollmentRefusal::HubNotEnrolled => "hub_not_enrolled",
            }))
    })
    .await;
    match outcome {
        Ok(Ok(Ok(enrollment))) => with_cors(Json(serde_json::json!({"account_enrollment": enrollment})).into_response(), &headers),
        Ok(Ok(Err(code))) => {
            let status = match code {
                "issuer_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
                "hub_not_enrolled" => StatusCode::CONFLICT,
                _ => StatusCode::FORBIDDEN,
            };
            with_cors((status, Json(serde_json::json!({"error": code}))).into_response(), &headers)
        }
        Ok(Err(error)) => with_cors(unauthorized_for(&error), &headers),
        Err(_) => with_cors((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"enrollment_failed"}))).into_response(), &headers),
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
        Ok(None) => return unauthorized(),
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
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
    /// Off by default (cas-6261): worker panes stay hidden on this stream.
    #[serde(default)]
    workers: bool,
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
fn audit_refused_pane_resize(auth: &Option<(AuthStore, AuthContext)>, session: &str, bytes: &[u8]) {
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

/// Every authenticated viewer of the session sees the operator conversation.
/// The reply's device id still routes delivery receipts to its addressee.
pub(super) fn operator_reply_allowed(
    auth: &Option<(AuthStore, AuthContext)>,
    bytes: &[u8],
) -> bool {
    operator_reply_receipt(bytes).is_none_or(|_| auth.is_some())
}

/// A live send belongs in every other authenticated viewer's thread. The
/// sender already has its optimistic bubble and receives MessageQueued.
fn operator_message_allowed(auth: &Option<(AuthStore, AuthContext)>, bytes: &[u8]) -> bool {
    let Ok(DaemonMessage::OperatorMessage(message)) =
        serde_json::from_slice::<DaemonMessage>(bytes)
    else {
        return true;
    };
    auth.as_ref().is_some_and(|(_, context)| {
        context.has(Scope::PaneRead) && context.device_id != message.device_id
    })
}

fn client_message_ref(bytes: &[u8]) -> Option<String> {
    let ClientMessage::SendMessage { client_ref, .. } =
        serde_json::from_slice::<ClientMessage>(bytes).ok()?
    else {
        return None;
    };
    client_ref
}

fn client_history_request_ref(bytes: &[u8]) -> Option<String> {
    let ClientMessage::ConversationHistoryRequest { request_id, .. } =
        serde_json::from_slice::<ClientMessage>(bytes).ok()?
    else {
        return None;
    };
    (!request_id.is_empty()).then_some(request_id)
}

fn client_correlation_ref(bytes: &[u8]) -> Option<String> {
    client_message_ref(bytes).or_else(|| client_history_request_ref(bytes))
}

/// What the browser is told when the session's daemon upstream could not take
/// a message. The wording avoids every word hub-web's refusal rules read as a
/// control or pairing problem, and says what happens next.
const UPSTREAM_UNAVAILABLE_MESSAGE: &str = "The session's daemon connection is reconnecting, so the message was not sent. Retry once the session is live again.";

/// Whether a `handle_client_message` failure is the retryable
/// upstream-unavailable case (cas-a0e2). Everything else stays a refusal.
fn is_upstream_unavailable(failure: &anyhow::Error) -> bool {
    failure
        .downcast_ref::<crate::hub::connector::UpstreamUnavailable>()
        .is_some()
}

/// The legacy per-session socket's answer to a failed client message:
/// `upstream_unavailable` (retryable, cas-a0e2) when the daemon upstream was
/// missing, otherwise the unchanged `forbidden` refusal.
fn legacy_send_error(failure: &anyhow::Error, client_ref: Option<&str>) -> serde_json::Value {
    if !is_upstream_unavailable(failure) {
        return legacy_forbidden_error(client_ref);
    }
    let mut error = serde_json::json!({
        "error": crate::hub::connector::UPSTREAM_UNAVAILABLE,
        "retryable": true,
        "message": UPSTREAM_UNAVAILABLE_MESSAGE,
    });
    if let Some(client_ref) = client_ref {
        error["client_ref"] = serde_json::Value::String(client_ref.to_owned());
    }
    error
}

/// The machine multiplex channel's answer to a failed client message; see
/// [`legacy_send_error`].
fn multiplex_send_error(
    failure: &anyhow::Error,
    session: &str,
    client_ref: Option<&str>,
) -> serde_json::Value {
    if !is_upstream_unavailable(failure) {
        return multiplex_forbidden_error(session, client_ref);
    }
    let mut error = serde_json::json!({
        "channel": format!("pty:{session}"),
        "error": {
            "code": crate::hub::connector::UPSTREAM_UNAVAILABLE,
            "retryable": true,
            "message": UPSTREAM_UNAVAILABLE_MESSAGE,
        },
    });
    if let Some(client_ref) = client_ref {
        error["error"]["client_ref"] = serde_json::Value::String(client_ref.to_owned());
    }
    error
}

/// Preserve the submitted reference on a legacy attach refusal without
/// changing the wire shape for clients that predate correlated sends.
fn legacy_forbidden_error(client_ref: Option<&str>) -> serde_json::Value {
    let mut error = serde_json::json!({"error": "forbidden"});
    if let Some(client_ref) = client_ref {
        error["client_ref"] = serde_json::Value::String(client_ref.to_owned());
    }
    error
}

/// Preserve the submitted reference inside the multiplex channel's structured
/// error envelope. The browser uses it to reject only the matching send.
fn multiplex_forbidden_error(session: &str, client_ref: Option<&str>) -> serde_json::Value {
    let mut error = serde_json::json!({
        "channel": format!("pty:{session}"),
        "error": {"code": "forbidden"},
    });
    if let Some(client_ref) = client_ref {
        error["error"]["client_ref"] = serde_json::Value::String(client_ref.to_owned());
    }
    error
}

/// MessageQueued and correlated Error frames share one daemon upstream, so
/// the hub filters them by the authenticated socket that submitted the ref.
pub(crate) fn correlated_daemon_frame_allowed(
    pending: &mut HashSet<(String, String)>,
    session: &str,
    bytes: &[u8],
) -> bool {
    if let Ok(DaemonMessage::MessageQueued { client_ref, .. }) =
        serde_json::from_slice::<DaemonMessage>(bytes)
    {
        return client_ref
            .is_some_and(|client_ref| pending.remove(&(session.to_owned(), client_ref)));
    }
    if let Ok(DaemonMessage::Error {
        client_ref: Some(client_ref),
        ..
    }) = serde_json::from_slice::<DaemonMessage>(bytes)
    {
        return pending.remove(&(session.to_owned(), client_ref));
    }
    if let Ok(DaemonMessage::ConversationHistory { request_id, .. }) =
        serde_json::from_slice::<DaemonMessage>(bytes)
    {
        return pending.remove(&(session.to_owned(), request_id));
    }
    true
}

/// Emit only bounded metadata for the private history response. The request
/// id and row counts make a missing relay observable without logging prompts,
/// replies, or credential material.
pub(crate) fn conversation_history_summary(bytes: &[u8]) -> Option<(String, usize, usize, bool)> {
    let DaemonMessage::ConversationHistory {
        request_id,
        messages,
        replies,
        has_earlier,
        ..
    } = serde_json::from_slice::<DaemonMessage>(bytes).ok()?
    else {
        return None;
    };
    Some((request_id, messages.len(), replies.len(), has_earlier))
}

fn trace_conversation_history_relay(session: &str, bytes: &[u8]) {
    let Some((request_id, messages, replies, has_earlier)) = conversation_history_summary(bytes)
    else {
        return;
    };
    tracing::info!(
        session,
        %request_id,
        messages,
        replies,
        has_earlier,
        "relayed Commander conversation history response"
    );
}

fn operator_reply_receipt(bytes: &[u8]) -> Option<(i64, String)> {
    let DaemonMessage::OperatorReply {
        notification_id,
        device_id,
        ..
    } = serde_json::from_slice::<DaemonMessage>(bytes).ok()?
    else {
        return None;
    };
    Some((notification_id, device_id))
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
    // Scoped to this authenticated machine socket; paired devices cannot see
    // one another's MessageQueued acknowledgments.
    let mut pending_message_refs = HashSet::<(String, String)>::new();
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
                    if !operator_reply_allowed(&auth, &frame.bytes) {
                        continue;
                    }
                    if !operator_message_allowed(&auth, &frame.bytes) {
                        continue;
                    }
                    if !correlated_daemon_frame_allowed(
                        &mut pending_message_refs,
                        &session,
                        &frame.bytes,
                    ) {
                        continue;
                    }
                    trace_conversation_history_relay(&session, &frame.bytes);
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
                        let mut worker_gate = super::WorkerGate::new(envelope.workers);
                        let handle = tokio::spawn(async move {
                            loop {
                                match viewer.recv().await {
                                    Ok(frame) => {
                                        let Some(frame) = worker_gate.admit(frame) else { continue };
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
                    let client_ref = client_correlation_ref(&bytes);
                    if let Some(client_ref) = client_ref.as_deref() {
                        pending_message_refs.insert((session.clone(), client_ref.to_owned()));
                    }
                    if let Err(failure) = handle_client_message(&state.connector, &session, &auth, &bytes).await {
                        if let Some(client_ref) = client_ref.as_deref() {
                            pending_message_refs.remove(&(session.clone(), client_ref.to_owned()));
                        }
                        let error = multiplex_send_error(&failure, &session, client_ref.as_deref());
                        if sink.send(Message::Text(error.to_string().into())).await.is_err() { break; }
                        // cas-0653: a re-subscribe is ignored while this
                        // session's subscription exists, so drop it and say
                        // the stream closed. The browser then subscribes
                        // again, `attach` restarts the daemon upstream, and
                        // the held send goes out on that live attach.
                        if is_upstream_unavailable(&failure) {
                            if let Some(handle) = subscriptions.remove(&session) {
                                handle.abort();
                            }
                            let closed = serde_json::json!({"channel":format!("pty:{session}"),"closed":true});
                            if sink.send(Message::Text(closed.to_string().into())).await.is_err() { break; }
                        }
                    }
                }
            },
            event = async {
                if events_subscribed {
                    machine_events.recv().await
                } else {
                    futures_util::future::pending().await
                }
            } => {
                match event {
                    Ok(event) => {
                        let envelope = serde_json::json!({"channel":"events","event":event});
                        if sink.send(Message::Text(envelope.to_string().into())).await.is_err() { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        let request_id = auth.as_ref().map(|(_, context)| context.request_id.clone())
                            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                        if let Some((store, _)) = auth.as_ref() {
                            let _ = store.audit_connection("events", false, 200, &request_id, Some("viewer_lagged"), skipped);
                        }
                        let envelope = serde_json::json!({"channel":"events","event":{
                            "kind":"viewer_lagged", "skipped":skipped, "request_id":request_id,
                        }});
                        if sink.send(Message::Text(envelope.to_string().into())).await.is_err() { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => { events_subscribed = false; }
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
        Ok(None) => return unauthorized(),
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
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
        Ok(None) => return unauthorized(),
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
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
        Ok(None) => return unauthorized(),
        Err(error) => return with_cors(unauthorized_for(&error), &headers),
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

/// A 401 that says why (cas-d636). An authentication refusal carries its
/// machine-readable reason, whether a fresh proof can succeed, and the hub's
/// clock (so a device whose clock drifted can correct its proofs), in the
/// body and as RFC 9449 `WWW-Authenticate: DPoP error=...`. Any other failure
/// stays the bare 401.
fn unauthorized_for(error: &anyhow::Error) -> Response {
    let Some(refusal) = error.downcast_ref::<super::AuthRefusal>().copied() else {
        return unauthorized();
    };
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "unauthorized",
            "reason": refusal.code(),
            "retryable": refusal.retryable(),
            "server_time": chrono::Utc::now().timestamp(),
        })),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&format!(
        "DPoP error=\"{}\", error_description=\"{}\"",
        refusal.dpop_error(),
        refusal.code()
    )) {
        response.headers_mut().insert("www-authenticate", value);
    }
    response.headers_mut().insert(
        "access-control-expose-headers",
        HeaderValue::from_static("WWW-Authenticate, X-Cas-Request-Id"),
    );
    response.headers_mut().insert("x-cas-refusal", HeaderValue::from_static(super::connection_recovery::refusal(refusal.code())));
    response
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

/// cassy#910: an artifact without a view URL answers with a status and a
/// stable code Commander can put into words; nothing internal leaks.
#[cfg(test)]
mod artifact_view_tests {
    use super::*;
    use crate::artifacts::ViewError;
    use crate::artifacts::cloud::ViewFailure;

    #[test]
    fn each_reason_an_artifact_cannot_be_viewed_has_its_own_status() {
        let cases = [
            (
                ViewError::NotInCloud {
                    status: "local".to_string(),
                },
                StatusCode::CONFLICT,
            ),
            (ViewError::NotLoggedIn, StatusCode::SERVICE_UNAVAILABLE),
            (
                ViewError::Cloud(ViewFailure::NotCommitted {
                    status: Some("pending".to_string()),
                    interaction: "GET …".to_string(),
                }),
                StatusCode::CONFLICT,
            ),
            (
                ViewError::Cloud(ViewFailure::NotFound {
                    interaction: "GET …".to_string(),
                }),
                StatusCode::NOT_FOUND,
            ),
            (
                ViewError::Cloud(ViewFailure::NotLive {
                    reason: "501".to_string(),
                }),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ViewError::Cloud(ViewFailure::Failed {
                    reason: "boom".to_string(),
                    interaction: "GET …".to_string(),
                }),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ViewError::Unknown("art-x".to_string()),
                StatusCode::NOT_FOUND,
            ),
            (
                ViewError::Store("disk".to_string()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (error, expected) in cases {
            let label = format!("{error:?}");
            assert_eq!(artifact_view_error(error).status(), expected, "{label}");
        }
    }
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

    #[test]
    fn legacy_forbidden_refusal_round_trips_client_ref_and_preserves_legacy_shape() {
        assert_eq!(
            legacy_forbidden_error(Some("send-42")),
            serde_json::json!({"error": "forbidden", "client_ref": "send-42"})
        );
        assert_eq!(
            legacy_forbidden_error(None),
            serde_json::json!({"error": "forbidden"})
        );
    }

    #[test]
    fn multiplex_forbidden_refusal_round_trips_client_ref_and_preserves_legacy_shape() {
        assert_eq!(
            multiplex_forbidden_error("factory-a", Some("send-42")),
            serde_json::json!({
                "channel": "pty:factory-a",
                "error": {"code": "forbidden", "client_ref": "send-42"},
            })
        );
        assert_eq!(
            multiplex_forbidden_error("factory-a", None),
            serde_json::json!({
                "channel": "pty:factory-a",
                "error": {"code": "forbidden"},
            })
        );
    }

    /// cas-a0e2: a missing daemon upstream is answered with the retryable
    /// `upstream_unavailable` code on both channels, carrying the client_ref;
    /// any other failure keeps the `forbidden` refusal byte for byte.
    #[test]
    fn upstream_unavailable_is_a_distinct_retryable_code_cas_a0e2() {
        let unavailable = anyhow::Error::new(crate::hub::connector::UpstreamUnavailable {
            reason: "session upstream closed",
        });
        assert_eq!(
            legacy_send_error(&unavailable, Some("send-42")),
            serde_json::json!({
                "error": "upstream_unavailable",
                "retryable": true,
                "message": UPSTREAM_UNAVAILABLE_MESSAGE,
                "client_ref": "send-42",
            })
        );
        assert_eq!(
            multiplex_send_error(&unavailable, "factory-a", Some("send-42")),
            serde_json::json!({
                "channel": "pty:factory-a",
                "error": {
                    "code": "upstream_unavailable",
                    "retryable": true,
                    "message": UPSTREAM_UNAVAILABLE_MESSAGE,
                    "client_ref": "send-42",
                },
            })
        );
        // The browser's refusal rules must not read it as a control problem.
        for word in [
            "forbidden",
            "control",
            "lease",
            "denied",
            "permission",
            "observ",
        ] {
            assert!(
                !UPSTREAM_UNAVAILABLE_MESSAGE
                    .to_ascii_lowercase()
                    .contains(word),
                "{word}"
            );
        }

        let refused = anyhow::anyhow!("authorization refused");
        assert_eq!(
            legacy_send_error(&refused, Some("send-42")),
            legacy_forbidden_error(Some("send-42"))
        );
        assert_eq!(
            multiplex_send_error(&refused, "factory-a", None),
            multiplex_forbidden_error("factory-a", None)
        );
    }

    #[test]
    fn message_queued_reaches_only_the_socket_that_submitted_its_ref() {
        let mut pending = HashSet::from([("factory-a".to_owned(), "send-42".to_owned())]);
        let queued = serde_json::to_vec(&DaemonMessage::MessageQueued {
            client_ref: Some("send-42".to_owned()),
            notification_id: 812,
            target: "patient-pelican-9".to_owned(),
            stamped: true,
            device_label: None,
        })
        .unwrap();
        assert!(correlated_daemon_frame_allowed(
            &mut pending,
            "factory-a",
            &queued
        ));
        assert!(
            pending.is_empty(),
            "receipt is single-delivery to its submitter"
        );
        assert!(!correlated_daemon_frame_allowed(
            &mut pending,
            "factory-a",
            &queued
        ));

        let error = serde_json::to_vec(&DaemonMessage::Error {
            message: "enqueue failed".to_owned(),
            client_ref: Some("send-99".to_owned()),
        })
        .unwrap();
        pending.insert(("factory-a".to_owned(), "send-99".to_owned()));
        assert!(correlated_daemon_frame_allowed(
            &mut pending,
            "factory-a",
            &error
        ));
    }

    #[test]
    fn live_operator_send_reaches_only_other_authenticated_viewers() {
        let temp = crate::test_support::private_hub_tempdir();
        let store = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
        let scopes: std::collections::BTreeSet<Scope> = [Scope::PaneRead].into_iter().collect();
        let sender = AuthContext::test_fixture("phone", "https://controller.example", scopes.clone());
        let viewer = AuthContext::test_fixture("desktop", "https://controller.example", scopes);
        let frame = serde_json::to_vec(&DaemonMessage::OperatorMessage(
            crate::ui::factory::ConversationHistoryMessage {
                notification_id: 41,
                target: "supervisor".into(),
                text: "Question".into(),
                state: "sending".into(),
                stamped: true,
                reply_to: None,
                device_id: "phone".into(),
                operator_label: Some("Pixel 10".into()),
                session: "factory-a".into(),
                at: "2026-09-29T16:00:00Z".into(),
            },
        )).unwrap();
        assert!(!operator_message_allowed(&None, &frame));
        assert!(!operator_message_allowed(&Some((store.clone(), sender)), &frame));
        assert!(operator_message_allowed(&Some((store, viewer)), &frame));
    }
}

#[cfg(test)]
mod catalog_visibility_tests {
    use super::*;

    #[test]
    fn normal_catalog_requires_fresh_reachable_supervisor() {
        let live = super::super::fixture_session("live");
        let mut dead = live.clone();
        dead.name = "dead".into();
        dead.dormant = true;
        let mut empty = live.clone();
        empty.name = "empty".into();
        empty.workers.clear();
        let mut missing = live.clone();
        missing.name = "missing".into();
        missing.liveness = super::super::DaemonLiveness::MissingEndpoint;
        let all = vec![live, dead, empty, missing];
        let visible = supervisor_sessions(all.clone(), false, false);
        assert_eq!(
            visible.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["live", "empty"]
        );
        assert_eq!(supervisor_sessions(all, false, true).len(), 4);
    }
}

/// cas-a0e2: the send path when the session's daemon upstream is missing.
#[cfg(test)]
mod upstream_unavailable_tests {
    use super::*;
    use crate::hub::SessionMultiplexer;

    /// The 2026-09-26 22:52Z field shape: a controlling device's send
    /// reached the hub while no daemon upstream could take it. It came back
    /// `forbidden` with no audit row. It now fails with the retryable
    /// upstream error and leaves an `unavailable` audit row.
    #[tokio::test]
    async fn a_send_with_no_daemon_upstream_is_audited_and_retryable_cas_a0e2() {
        let temp = crate::test_support::private_hub_tempdir();
        let root = temp.path().join("hub");
        let auth = AuthStore::open(&root, "machine-test").unwrap();
        let now = chrono::Utc::now();
        let scopes: std::collections::BTreeSet<Scope> =
            [Scope::MessageSend, Scope::PaneRead].into_iter().collect();
        let invitation = auth
            .mint_pairing("https://controller.example", scopes.clone(), now)
            .unwrap();
        let exchange = PairingExchange::test_fixture(
            invitation.token,
            "machine-test",
            "https://controller.example",
            scopes.clone(),
        );
        let credential = auth.exchange_pairing(exchange, now).unwrap();
        let phone = AuthContext {
            device_id: credential.device_id,
            credential_id: credential.credential_id,
            device_label: "phone".into(),
            operator_label: "Daniel".into(),
            controller_origin: "https://controller.example".into(),
            scopes,
            request_id: "request-a0e2".into(),
        };
        auth.acquire_lease(&phone, "factory-a", now).unwrap();
        let connector = DaemonConnector::new(SessionMultiplexer::new(8), MachineEventBus::new(8));
        let frame = serde_json::json!({
            "SendMessage": {
                "client_ref": "send-a0e2",
                "target": "patient-pelican-9",
                "text": "Do the burn down",
                "summary": "Cassy Cloud message",
                "urgent": false,
                "attribution": {
                    "device_id": null, "credential_id": null, "device_label": null,
                    "operator_label": null, "controller_origin": null, "request_id": null
                }
            }
        });
        let bytes = serde_json::to_vec(&frame).unwrap();

        let failure = handle_client_message(
            &connector,
            "factory-a",
            &Some((auth.clone(), phone.clone())),
            &bytes,
        )
        .await
        .expect_err("no daemon upstream can take the message");
        assert!(is_upstream_unavailable(&failure), "{failure:#}");
        assert_eq!(
            multiplex_send_error(&failure, "factory-a", Some("send-a0e2"))["error"]["code"],
            "upstream_unavailable"
        );

        let audit = std::fs::read_to_string(root.join("audit.jsonl")).unwrap();
        let row = audit
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|row| row["outcome"] == "unavailable")
            .unwrap_or_else(|| panic!("no unavailable audit row in:\n{audit}"));
        assert_eq!(row["action"], "websocket_mutation");
        assert_eq!(row["target_session"], "factory-a");
        assert_eq!(row["request_id"], "request-a0e2");

        // A device without the lease is still refused as before, and that
        // refusal stays `forbidden`.
        auth.release_lease(&phone, "factory-a", now).unwrap();
        let refused = handle_client_message(
            &connector,
            "factory-a",
            &Some((auth.clone(), phone)),
            &bytes,
        )
        .await
        .expect_err("no lease");
        assert!(!is_upstream_unavailable(&refused));
        assert_eq!(
            legacy_send_error(&refused, Some("send-a0e2"))["error"],
            "forbidden"
        );
    }
}
