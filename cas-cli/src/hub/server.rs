use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt, stream};
use serde::{Deserialize, Serialize};

use super::{
    DaemonConnector, HealthResponse, HubAction, HubAuthorizer, HubRequest, HubSession,
    MachineEventBus, MachineIdentity, SessionCatalog, SessionReadModel, ViewerRecvError,
};

#[derive(Clone)]
pub struct HubState<R: SessionReadModel> {
    catalog: SessionCatalog<R>,
    authorizer: Arc<dyn HubAuthorizer>,
    machine: MachineIdentity,
    connector: DaemonConnector,
    events: MachineEventBus,
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
        }
    }
}

pub fn router<R: SessionReadModel>(state: HubState<R>) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/machine", get(machine::<R>))
        .route("/v1/sessions", get(sessions::<R>))
        .route("/v1/events", get(events::<R>))
        .route("/v1/sessions/{session}/status", get(status::<R>))
        .route("/v1/sessions/{session}/attach", get(attach::<R>))
        .with_state(state)
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
    if !authorized(&state, HubAction::MachineRead, &headers) {
        return unauthorized();
    }
    Json(MachineResponse {
        schema_version: super::HUB_SCHEMA_VERSION,
        machine_id: state.machine.id,
        version: env!("CARGO_PKG_VERSION"),
        capabilities: &["session_index", "daemon_attach", "machine_events"],
    })
    .into_response()
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
    if !authorized(&state, HubAction::SessionRead, &headers) {
        return unauthorized();
    }
    match state.catalog.list().await {
        Ok(sessions) => Json(SessionsResponse {
            schema_version: super::HUB_SCHEMA_VERSION,
            sessions,
        })
        .into_response(),
        Err(error) => internal_error(error),
    }
}

async fn events<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, HubAction::SessionRead, &headers) {
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
    Sse::new(output)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn status<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, HubAction::SessionRead, &headers) {
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
        Ok(Ok(status)) => Json(status).into_response(),
        Ok(Err(_)) => generic_not_found(),
        Err(error) => internal_error(error.into()),
    }
}

#[derive(Debug, Default, Deserialize)]
struct AttachQuery {
    #[serde(default)]
    panes: String,
}

async fn attach<R: SessionReadModel>(
    State(state): State<HubState<R>>,
    Path(session): Path<String>,
    Query(query): Query<AttachQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !authorized(&state, HubAction::PaneRead, &headers) {
        return unauthorized();
    }
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
        .on_upgrade(move |socket| proxy_socket(socket, connector, session, port, panes))
        .into_response()
}

async fn proxy_socket(
    socket: WebSocket,
    connector: DaemonConnector,
    session: String,
    port: u16,
    panes: Vec<String>,
) {
    let Ok(mut viewer) = connector.attach(&session, port, panes).await else {
        return;
    };
    let (mut sink, mut source) = socket.split();
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
                // H1 is a read-only pre-auth transport. H2 replaces the authorizer
                // and adds scoped mutation routing without changing this fan-out.
                Some(Ok(_)) => {
                    let _ = sink.send(Message::Text(
                        r#"{"error":"mutations_require_h2_authorization"}"#.into()
                    )).await;
                    break;
                }
            }
        }
    }
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
