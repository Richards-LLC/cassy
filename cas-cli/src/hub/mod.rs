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

use crate::store::{find_cas_root_ignoring_env, open_agent_store};
use crate::ui::factory::{DaemonMessage, SessionInfo, SessionManager};

mod attention;
mod auth;
mod connector;
mod death;
mod discovery;
mod events;
mod connection_recovery;
mod identity;
pub(crate) mod observation;
pub mod launch_env;
pub mod operator_inbox;
pub mod projects;
mod runtime;
mod server;
pub(crate) mod state;
mod tailscale;
mod worker_gate;

pub(crate) use attention::spawn_attention_enricher;
pub use auth::{
    AUDIT_HEALTH_FILE, AUDIT_LOG_FILE, AuditHealth, AuditWriterReport, AuthRefusal, audit_writer_report,
    read_audit_health,
};
pub use auth::{
    AccountEnrollment, InstallationAction, InstallationProof, AuthContext, AuthStore, DeviceCredential, DeviceSession, DeviceSummary, LeaseSummary,
    PairingExchange, PairingExchangeError, PairingInvitation, PairingPrefill, PublicJwk, Scope,
    WsTicket, required_scope,
};
pub(crate) use auth::PairingInvitationTarget;
pub use connector::DaemonConnector;
pub use death::{DaemonExitEvidenceStore, DaemonExitReceipt, DaemonIdentity};
#[cfg(unix)]
pub(crate) use death::{supervise_forked_daemon, supervise_spawned_daemon};
#[cfg(unix)]
pub(crate) use death::reap_spawned_daemon;
pub use discovery::{CloudDeviceSuggestion, load_cloud_device_suggestions};
pub use events::{
    AttentionAction, AttentionEnrichment, AttentionSeverity, MachineEvent, MachineEventBus,
    MachineEventKind, SessionAttentionContext,
};
pub use identity::{MachineIdentity, MachineIdentityStore};
pub use runtime::{
    HubInstanceLock, HubLockHolder, HubLockOwner, HubProcessRecord, HubRuntimePaths,
    processes_holding_file,
};
pub use server::{HubState, router};
pub(crate) use state::ensure_private_dir;
pub use tailscale::{TailscaleServeManager, TailscaleServeReceipt};
pub use worker_gate::WorkerGate;

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
// Public health keeps its two-key readiness contract. Installation capability
// discovery requires an invitation commitment at /v1/auth/pairing/protocol.
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
    /// True when the session metadata remains but its registered supervisor
    /// is no longer live. Dormant sessions are hidden from Commander by
    /// default, but can be revealed for recovery.
    #[serde(default)]
    pub dormant: bool,
    /// When the session last did anything (cas-55a4): its newest queue row,
    /// Commander-facing or not. Lets Commander tell apart several live
    /// sessions of one project and show activity for a session that has not
    /// written to Commander yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    /// Who that newest row was from and to, e.g. `supervisor → worker-1`.
    /// Never the row's content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    /// When the session started (its metadata's `created_at`). Commander
    /// marks the newest of a project's sessions as most recent when none of
    /// them has activity yet (cas-6acf).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip)]
    pub daemon_identity: Option<DaemonIdentity>,
}

pub trait SessionReadModel: Clone + Send + Sync + 'static {
    fn list_sessions(&self) -> Result<Vec<HubSession>>;
}

/// The machine's real session catalog, read from session files and each
/// project's agent registry.
///
/// Every pass reads the agent registry (`<project>/.cas/cas.db`) of every
/// listed session. Before cas-e335 each pass opened that database and the
/// last store drop closed it again, once a second per project. On macOS a
/// close (`sqlite3WalClose`) racing a reopen (`sqlite3BtreeOpen`) of the same
/// file deadlocked inside SQLite's unix VFS and wedged the hub. `shared_db`
/// now owns every close, and the model also holds one registry handle per
/// listed project while that project has a listed session, so the hub's
/// registries never go idle and are never swept.
#[derive(Clone, Default)]
pub struct LocalSessionReadModel {
    registries: Arc<std::sync::Mutex<HashMap<std::path::PathBuf, PinnedRegistry>>>,
    /// Last activity per session, read at most once per
    /// [`LAST_ACTIVITY_TTL`] so a device's five-second catalog refresh does
    /// not query every project's queue each time.
    activity: Arc<std::sync::Mutex<HashMap<String, CachedActivity>>>,
}

/// How long a session's last activity is reused before it is read again.
const LAST_ACTIVITY_TTL: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone)]
struct CachedActivity {
    read_at: std::time::Instant,
    activity: Option<(String, String)>,
}

impl std::fmt::Debug for LocalSessionReadModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pinned = self
            .registries
            .lock()
            .map(|registries| registries.len())
            .unwrap_or_default();
        formatter
            .debug_struct("LocalSessionReadModel")
            .field("pinned_registries", &pinned)
            .finish()
    }
}

/// One project's agent registry, held open across catalog passes.
struct PinnedRegistry {
    _store: Arc<dyn crate::store::AgentStore>,
    /// Identity of `cas.db` when the handle was opened. A database replaced or
    /// deleted underneath the hub is reopened instead of read stale.
    identity: Option<FileIdentity>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

fn file_identity(path: &std::path::Path) -> Option<FileIdentity> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Some(FileIdentity {
            device: 0,
            inode: 0,
        })
    }
}

fn session_cas_root(session: &SessionInfo) -> Option<std::path::PathBuf> {
    let project_dir = session.metadata.project_dir.as_deref()?;
    find_cas_root_ignoring_env(std::path::Path::new(project_dir)).ok()
}

impl LocalSessionReadModel {
    /// Project discovered sessions onto the wire shape, holding each listed
    /// project's registry open for the whole pass and until the project stops
    /// being listed.
    fn project(&self, sessions: &[SessionInfo]) -> Vec<HubSession> {
        self.pin_registries(sessions);
        sessions
            .iter()
            .map(|session| {
                let mut projected = hub_session(session);
                if let Some((at, label)) = self.last_activity(session) {
                    projected.last_activity_at = Some(at);
                    projected.last_activity = Some(label);
                }
                projected
            })
            .collect()
    }

    fn last_activity(&self, session: &SessionInfo) -> Option<(String, String)> {
        let mut cache = self
            .activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = cache.get(&session.name)
            && cached.read_at.elapsed() < LAST_ACTIVITY_TTL
        {
            return cached.activity.clone();
        }
        let activity = session_cas_root(session).and_then(|cas_root| {
            read_last_activity(&cas_root, &session.name)
        });
        cache.insert(
            session.name.clone(),
            CachedActivity {
                read_at: std::time::Instant::now(),
                activity: activity.clone(),
            },
        );
        activity
    }

    fn pin_registries(&self, sessions: &[SessionInfo]) {
        let mut pinned = self
            .registries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Handles move from `previous` to `pinned` as their projects are seen
        // again. What is left over belongs to projects no longer listed and is
        // released at the end of the pass, on this thread. `SessionCatalog`
        // runs one pass at a time, so a release never races a pass's open.
        let mut previous = std::mem::take(&mut *pinned);
        for cas_root in sessions.iter().filter_map(session_cas_root) {
            if pinned.contains_key(&cas_root) {
                continue;
            }
            let identity = file_identity(&cas_root.join("cas.db"));
            if let Some(registry) = previous.remove(&cas_root) {
                if registry.identity.is_some() && registry.identity == identity {
                    pinned.insert(cas_root, registry);
                    continue;
                }
                // Close the stale handle before reopening, so the pool cannot
                // hand the replaced database's connection back.
                drop(registry);
            }
            if let Ok(store) = open_agent_store(&cas_root) {
                let identity = file_identity(&cas_root.join("cas.db"));
                pinned.insert(
                    cas_root,
                    PinnedRegistry {
                        _store: store,
                        identity,
                    },
                );
            }
        }
        drop(pinned);
        drop(previous);
    }
}

/// A session's newest queue row as `(rfc3339, "from → to")`. The store is
/// opened without its schema pass: the catalog only reads, and a project whose
/// queue predates the session index still answers, just more slowly.
fn read_last_activity(cas_root: &std::path::Path, session: &str) -> Option<(String, String)> {
    use cas_store::PromptQueueStore;
    let store = cas_store::SqlitePromptQueueStore::open(cas_root).ok()?;
    let row = store.latest_session_activity(session).ok()??;
    Some((
        row.created_at.to_rfc3339(),
        activity_label(&row.source, &row.target),
    ))
}

/// `from → to` for a queue row, naming Commander instead of the operator's
/// device label so the catalog never carries a paired device's name.
pub(crate) fn activity_label(source: &str, target: &str) -> String {
    fn party(name: &str) -> &str {
        if name.starts_with("commander:") || name.eq_ignore_ascii_case("operator") {
            "Commander"
        } else if name == "terminal-history" {
            // Input typed at the supervisor's terminal, recorded for history.
            "supervisor"
        } else {
            // `lifecycle-wake:worker-died:8290` reads as its kind.
            name.split(':').next().unwrap_or(name)
        }
    }
    format!("{} → {}", party(source), party(target))
}

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
        dormant: !has_live_supervisor(session),
        last_activity_at: None,
        last_activity: None,
        started_at: Some(session.metadata.created_at.clone()).filter(|at| !at.trim().is_empty()),
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

/// A session's metadata names the supervisor, but does not prove that the
/// pane's harness is responsive. Require the worker_status fresh-heartbeat
/// band (a surviving process alone is not a reachable conversation), scoped to the
/// recorded supervisor name.
fn has_live_supervisor(session: &SessionInfo) -> bool {
    let supervisor_name = session.metadata.supervisor.name.trim();
    let Some(project_dir) = session.metadata.project_dir.as_deref() else {
        return false;
    };
    if supervisor_name.is_empty() {
        return false;
    }
    let Ok(cas_root) = find_cas_root_ignoring_env(std::path::Path::new(project_dir)) else {
        return false;
    };
    let Ok(agent_store) = open_agent_store(&cas_root) else {
        return false;
    };
    let Ok(agents) = agent_store.list(None) else {
        return false;
    };
    agents.iter().any(|agent| {
        agent.role == cas_types::AgentRole::Supervisor
            && agent.factory_session.as_deref() == Some(session.name.as_str())
            && agent.name == supervisor_name
            && crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness_with(
                agent,
                false,
                crate::mcp::tools::service::agent_liveness::WORKER_STALE_SECS,
            )
            .is_live()
    })
}

impl SessionReadModel for LocalSessionReadModel {
    fn list_sessions(&self) -> Result<Vec<HubSession>> {
        Ok(self.project(&SessionManager::new().list_sessions()?))
    }
}

/// Outcome of one catalog read. Both sides are shared among its callers.
type SessionReadOutcome = std::result::Result<Arc<Vec<HubSession>>, Arc<String>>;

/// One catalog read, shared by every caller that arrives while it runs.
type SharedSessionRead =
    futures_util::future::Shared<futures_util::future::BoxFuture<'static, SessionReadOutcome>>;

/// How long one `SessionCatalog::list` caller waits for the shared read.
/// The read itself keeps running on its blocking thread; a later caller joins
/// it instead of starting another.
const SESSION_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The hub's view of the machine's factory sessions.
///
/// `SessionReadModel::list_sessions` is synchronous and reaches SQLite (the
/// agent registry of every project) on each call. Run inline on a Tokio worker,
/// a stalled read pins that worker; the hub's once-a-second catalog poller plus
/// a few `/v1/sessions` reads pinned every worker, the IO reactor starved, and
/// the process stayed alive holding `hub.lock` while `/v1/health` stopped
/// answering on loopback and Tailscale. Reads therefore run on the blocking
/// pool, and concurrent callers share one in-flight read (single-flight), so a
/// stalled read costs one blocking thread instead of one worker per caller.
#[derive(Clone)]
pub struct SessionCatalog<R: SessionReadModel> {
    read_model: R,
    in_flight: Arc<std::sync::Mutex<Option<SharedSessionRead>>>,
}

impl<R: SessionReadModel> SessionCatalog<R> {
    pub fn new(read_model: R) -> Self {
        Self {
            read_model,
            in_flight: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub async fn list(&self) -> Result<Vec<HubSession>> {
        let read = self.shared_read();
        match tokio::time::timeout(SESSION_READ_TIMEOUT, read).await {
            Ok(Ok(sessions)) => Ok(sessions.as_ref().clone()),
            Ok(Err(error)) => Err(anyhow::anyhow!("{error}")),
            Err(_) => Err(anyhow::anyhow!(
                "session catalog read did not finish within {}s",
                SESSION_READ_TIMEOUT.as_secs()
            )),
        }
    }

    /// Join the read in flight, or start one when none is running. A finished
    /// read is never reused: a caller either joins the read already running
    /// or starts a fresh one.
    fn shared_read(&self) -> SharedSessionRead {
        use futures_util::FutureExt;

        let mut slot = self
            .in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(read) = slot.as_ref()
            && read.peek().is_none()
        {
            return read.clone();
        }
        let read_model = self.read_model.clone();
        let read: SharedSessionRead = async move {
            match tokio::task::spawn_blocking(move || read_model.list_sessions()).await {
                Ok(Ok(sessions)) => Ok(Arc::new(sessions)),
                Ok(Err(error)) => Err(Arc::new(format!("{error:#}"))),
                Err(join) => Err(Arc::new(format!("session catalog read failed: {join}"))),
            }
        }
        .boxed()
        .shared();
        *slot = Some(read.clone());
        read
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
        dormant: false,
        last_activity_at: None,
        last_activity: None,
        started_at: None,
        daemon_identity: None,
    }
}

#[cfg(test)]
mod tests;
