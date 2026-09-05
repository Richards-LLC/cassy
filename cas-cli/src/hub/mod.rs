//! Machine-local Commander hub.
//!
//! The binding security and multiplexing contract lives in
//! `docs/specs/2026-08-08-commander-security-architecture.md`.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};

use crate::ui::factory::{DaemonMessage, SessionInfo, SessionManager};

mod attention;
mod auth;
mod connector;
mod death;
mod discovery;
mod events;
mod identity;
mod runtime;
mod server;
mod state;
mod tailscale;

pub(crate) use attention::spawn_attention_enricher;
pub use auth::{
    AuthContext, AuthStore, DeviceCredential, DeviceSession, DeviceSummary, LeaseSummary,
    PairingExchange, PairingExchangeError, PairingInvitation, PublicJwk, Scope, WsTicket,
    required_scope,
};
pub(crate) use auth::PairingInvitationTarget;
pub use connector::DaemonConnector;
pub use death::{DaemonExitEvidenceStore, DaemonExitReceipt, DaemonIdentity};
#[cfg(unix)]
pub(crate) use death::{supervise_forked_daemon, supervise_spawned_daemon};
pub use discovery::{CloudDeviceSuggestion, load_cloud_device_suggestions};
pub use events::{
    AttentionAction, AttentionEnrichment, AttentionSeverity, MachineEvent, MachineEventBus,
    MachineEventKind, SessionAttentionContext,
};
pub use identity::{MachineIdentity, MachineIdentityStore};
pub use runtime::{HubInstanceLock, HubProcessRecord, HubRuntimePaths};
pub use server::{HubState, router};
pub(crate) use state::ensure_private_dir;
pub use tailscale::{TailscaleServeManager, TailscaleServeReceipt};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineTransport {
    pub kind: String,
    pub public_url: Option<String>,
}

impl Default for MachineTransport {
    fn default() -> Self {
        Self {
            kind: "loopback".to_owned(),
            public_url: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineMetadata {
    pub transport: MachineTransport,
    pub cloud_devices: Vec<CloudDeviceSuggestion>,
}

pub const HUB_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_HUB_PORT: u16 = 4173;
pub const DEFAULT_VIEWER_QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubAction {
    Health,
    MachineRead,
    SessionRead,
    PaneRead,
    Mutation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubRequest {
    pub action: HubAction,
    pub origin: Option<String>,
}

impl HubRequest {
    pub fn health() -> Self {
        Self {
            action: HubAction::Health,
            origin: None,
        }
    }

    pub fn sessions(origin: Option<&str>) -> Self {
        Self {
            action: HubAction::SessionRead,
            origin: origin.map(str::to_owned),
        }
    }

    pub fn mutation(origin: Option<&str>) -> Self {
        Self {
            action: HubAction::Mutation,
            origin: origin.map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationDecision {
    Allow,
    Deny,
}

impl AuthorizationDecision {
    pub fn is_allowed(self) -> bool {
        self == Self::Allow
    }

    pub fn is_denied(self) -> bool {
        self == Self::Deny
    }
}

/// H2 replaces this policy with DPoP/scoped authorization. H1 deliberately
/// defines only the enforcement seam and never mints temporary credentials.
pub trait HubAuthorizer: Send + Sync + 'static {
    fn authorize(&self, request: &HubRequest) -> AuthorizationDecision;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PreAuthAuthorizer;

impl HubAuthorizer for PreAuthAuthorizer {
    fn authorize(&self, request: &HubRequest) -> AuthorizationDecision {
        if request.action == HubAction::Health {
            AuthorizationDecision::Allow
        } else {
            AuthorizationDecision::Deny
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportSecurity {
    Plaintext,
    Tls13,
    TrustedLoopbackTlsProxy,
}

pub fn validate_control_bind(addr: SocketAddr, transport: TransportSecurity) -> Result<()> {
    match transport {
        TransportSecurity::Plaintext if !addr.ip().is_loopback() => {
            anyhow::bail!("plaintext Commander hub control is restricted to loopback")
        }
        TransportSecurity::TrustedLoopbackTlsProxy if !addr.ip().is_loopback() => {
            anyhow::bail!("the hub behind a TLS proxy must itself bind loopback")
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthResponse {
    pub schema_version: u32,
    pub ready: bool,
}

impl HealthResponse {
    pub fn ready() -> Self {
        Self {
            schema_version: HUB_SCHEMA_VERSION,
            ready: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyFrame {
    pub bytes: Vec<u8>,
    pub pane_id: Option<String>,
    pub kind: ProxyFrameKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyFrameKind {
    Output,
    PaneKeyframe,
    Other,
}

pub fn proxy_frame(message: DaemonMessage) -> ProxyFrame {
    let pane_id = match &message {
        DaemonMessage::Output { pane_id, .. }
        | DaemonMessage::PaneKeyframe { pane_id, .. }
        | DaemonMessage::ScrollbackPage { pane_id, .. }
        | DaemonMessage::PaneExited { pane_id, .. }
        | DaemonMessage::PaneRemoved { pane_id } => Some(pane_id.clone()),
        DaemonMessage::PaneAdded { pane } => Some(pane.id.clone()),
        _ => None,
    };
    let kind = match &message {
        DaemonMessage::Output { .. } => ProxyFrameKind::Output,
        DaemonMessage::PaneKeyframe { .. } => ProxyFrameKind::PaneKeyframe,
        _ => ProxyFrameKind::Other,
    };
    ProxyFrame {
        bytes: serde_json::to_vec(&message).expect("DaemonMessage must serialize"),
        pane_id,
        kind,
    }
}

struct Viewer {
    panes: HashSet<String>,
    tx: mpsc::Sender<ProxyFrame>,
    lagged: Arc<AtomicU64>,
}

#[derive(Default)]
struct SessionFanout {
    upstream_starts: usize,
    snapshot: Option<ProxyFrame>,
    viewers: HashMap<usize, Viewer>,
}

#[derive(Default)]
struct MultiplexerState {
    sessions: HashMap<String, SessionFanout>,
}

#[derive(Clone)]
pub struct SessionMultiplexer {
    capacity: usize,
    next_viewer_id: Arc<AtomicUsize>,
    state: Arc<Mutex<MultiplexerState>>,
}

impl SessionMultiplexer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            next_viewer_id: Arc::new(AtomicUsize::new(1)),
            state: Arc::new(Mutex::new(MultiplexerState::default())),
        }
    }

    pub async fn subscribe<I, S>(&self, session: &str, panes: I) -> ViewerReceiver
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let viewer_id = self.next_viewer_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(self.capacity);
        let lagged = Arc::new(AtomicU64::new(0));
        let mut state = self.state.lock().await;
        let fanout = state
            .sessions
            .entry(session.to_owned())
            .or_insert_with(|| SessionFanout {
                upstream_starts: 1,
                snapshot: None,
                viewers: HashMap::new(),
            });
        let snapshot = fanout.snapshot.clone();
        fanout.viewers.insert(
            viewer_id,
            Viewer {
                panes: panes.into_iter().map(Into::into).collect(),
                tx,
                lagged: lagged.clone(),
            },
        );
        if let Some(snapshot) = snapshot {
            let _ = fanout
                .viewers
                .get(&viewer_id)
                .expect("viewer was just inserted")
                .tx
                .try_send(snapshot);
        }
        ViewerReceiver {
            rx,
            lagged,
            viewer_id,
            session: session.to_owned(),
            state: self.state.clone(),
        }
    }

    pub async fn publish(&self, session: &str, frame: ProxyFrame) -> Result<()> {
        let state = self.state.lock().await;
        let Some(fanout) = state.sessions.get(session) else {
            anyhow::bail!("session '{session}' has no upstream fan-out")
        };
        for viewer in fanout.viewers.values() {
            if frame
                .pane_id
                .as_ref()
                .is_some_and(|pane| !viewer.panes.is_empty() && !viewer.panes.contains(pane))
            {
                continue;
            }
            match viewer.tx.try_send(frame.clone()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    viewer.lagged.fetch_add(1, Ordering::Relaxed);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
        }
        Ok(())
    }

    pub async fn publish_snapshot(&self, session: &str, frame: ProxyFrame) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            let Some(fanout) = state.sessions.get_mut(session) else {
                anyhow::bail!("session '{session}' has no upstream fan-out")
            };
            fanout.snapshot = Some(frame.clone());
        }
        self.publish(session, frame).await
    }

    pub async fn upstream_start_count(&self, session: &str) -> usize {
        self.state
            .lock()
            .await
            .sessions
            .get(session)
            .map_or(0, |fanout| fanout.upstream_starts)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerRecvError {
    Lagged { skipped: u64 },
    Closed,
}

pub struct ViewerReceiver {
    rx: mpsc::Receiver<ProxyFrame>,
    lagged: Arc<AtomicU64>,
    viewer_id: usize,
    session: String,
    state: Arc<Mutex<MultiplexerState>>,
}

impl ViewerReceiver {
    pub async fn recv(&mut self) -> std::result::Result<ProxyFrame, ViewerRecvError> {
        let skipped = self.lagged.swap(0, Ordering::Relaxed);
        if skipped > 0 {
            // A queued terminal delta is no longer meaningful after any gap.
            // Discard the bounded backlog so the caller can request a fresh
            // authoritative keyframe instead of rendering seconds of stale IO.
            while self.rx.try_recv().is_ok() {}
            return Err(ViewerRecvError::Lagged { skipped });
        }
        self.rx.recv().await.ok_or(ViewerRecvError::Closed)
    }

    pub fn try_recv(&mut self) -> std::result::Result<ProxyFrame, ViewerRecvError> {
        let skipped = self.lagged.swap(0, Ordering::Relaxed);
        if skipped > 0 {
            while self.rx.try_recv().is_ok() {}
            return Err(ViewerRecvError::Lagged { skipped });
        }
        self.rx.try_recv().map_err(|_| ViewerRecvError::Closed)
    }
}

impl Drop for ViewerReceiver {
    fn drop(&mut self) {
        let state = self.state.clone();
        let session = self.session.clone();
        let viewer_id = self.viewer_id;
        if let Ok(mut state) = state.try_lock() {
            if let Some(fanout) = state.sessions.get_mut(&session) {
                fanout.viewers.remove(&viewer_id);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProcessExit {
    Code(i32),
    Signal(i32),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonDeathCause {
    CleanExit {
        code: i32,
    },
    ExitCode {
        code: i32,
    },
    Signal {
        signal: i32,
        name: Option<String>,
        core_dumped: Option<bool>,
    },
    TransportLost,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonDeathDiagnostic {
    pub cause: DaemonDeathCause,
    pub next_action: String,
}

pub fn diagnose_daemon_death(
    exit: Option<ProcessExit>,
    core_dumped: Option<bool>,
) -> DaemonDeathDiagnostic {
    let cause = match exit {
        Some(ProcessExit::Code(0)) => DaemonDeathCause::CleanExit { code: 0 },
        Some(ProcessExit::Code(code)) => DaemonDeathCause::ExitCode { code },
        Some(ProcessExit::Signal(signal)) => DaemonDeathCause::Signal {
            signal,
            name: signal_name(signal).map(str::to_owned),
            core_dumped,
        },
        None => DaemonDeathCause::Unknown,
    };
    let next_action = match &cause {
        DaemonDeathCause::Signal { signal: 4, .. } => {
            "Replace this Cassy binary with the portable release artifact for this machine, then \
             restart the factory session; preserve the daemon log and core dump for diagnosis."
        }
        _ => {
            "Inspect the factory daemon log and session metadata; do not infer a cause from a \
             closed socket alone."
        }
    };
    DaemonDeathDiagnostic {
        cause,
        next_action: next_action.into(),
    }
}

fn signal_name(signal: i32) -> Option<&'static str> {
    match signal {
        4 => Some("SIGILL"),
        6 => Some("SIGABRT"),
        9 => Some("SIGKILL"),
        11 => Some("SIGSEGV"),
        15 => Some("SIGTERM"),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonLiveness {
    Live,
    StaleMetadata,
    MissingEndpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubSession {
    pub name: String,
    pub project_dir: Option<String>,
    pub supervisor: String,
    pub workers: Vec<String>,
    pub epic_id: Option<String>,
    pub ws_port: Option<u16>,
    pub liveness: DaemonLiveness,
    #[serde(skip)]
    pub daemon_identity: Option<DaemonIdentity>,
}

pub trait SessionReadModel: Clone + Send + Sync + 'static {
    fn list_sessions(&self) -> Result<Vec<HubSession>>;
}

#[derive(Debug, Clone, Default)]
pub struct LocalSessionReadModel;

/// Project one discovered session onto the wire shape Commander consumes.
///
/// The roster comes from `SessionInfo::worker_names()` — the live agent
/// registry, the same source the factory TUI counts — because the session file
/// this metadata is read from carries an empty `workers[]` even for a session
/// running five of them, which made every session report "0 workers".
fn hub_session(session: &SessionInfo) -> HubSession {
    HubSession {
        name: session.name.clone(),
        project_dir: session.metadata.project_dir.clone(),
        supervisor: session.metadata.supervisor.name.clone(),
        workers: session.project_worker_names(),
        epic_id: session.metadata.epic_id.clone(),
        ws_port: session.metadata.ws_port,
        liveness: if !session.is_running {
            DaemonLiveness::StaleMetadata
        } else if session.metadata.ws_port.is_none() {
            DaemonLiveness::MissingEndpoint
        } else {
            DaemonLiveness::Live
        },
        daemon_identity: session
            .metadata
            .daemon_pid_starttime
            .map(|pid_starttime| DaemonIdentity {
                session: session.name.clone(),
                pid: session.metadata.daemon_pid,
                pid_starttime,
            }),
    }
}

impl SessionReadModel for LocalSessionReadModel {
    fn list_sessions(&self) -> Result<Vec<HubSession>> {
        Ok(SessionManager::new()
            .list_sessions()?
            .iter()
            .map(hub_session)
            .collect())
    }
}

#[derive(Clone)]
pub struct SessionCatalog<R: SessionReadModel> {
    read_model: R,
}

impl<R: SessionReadModel> SessionCatalog<R> {
    pub fn new(read_model: R) -> Self {
        Self { read_model }
    }

    pub async fn list(&self) -> Result<Vec<HubSession>> {
        self.read_model.list_sessions()
    }
}

#[cfg(test)]
#[derive(Clone)]
pub struct RecordingReadModel {
    sessions: Vec<HubSession>,
    reads: Arc<AtomicUsize>,
}

#[cfg(test)]
impl RecordingReadModel {
    pub fn with_sessions(sessions: Vec<HubSession>) -> Self {
        Self {
            sessions,
            reads: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn read_count(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }

    pub fn pty_write_count(&self) -> usize {
        0
    }

    pub fn model_call_count(&self) -> usize {
        0
    }

    pub fn logical_session_create_count(&self) -> usize {
        0
    }
}

#[cfg(test)]
impl SessionReadModel for RecordingReadModel {
    fn list_sessions(&self) -> Result<Vec<HubSession>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(self.sessions.clone())
    }
}

#[cfg(test)]
pub fn fixture_session(name: &str) -> HubSession {
    HubSession {
        name: name.to_owned(),
        project_dir: Some("/tmp/project".into()),
        supervisor: "supervisor".into(),
        workers: vec!["worker-1".into()],
        epic_id: None,
        ws_port: Some(12345),
        liveness: DaemonLiveness::Live,
        daemon_identity: None,
    }
}

#[cfg(test)]
mod tests;
