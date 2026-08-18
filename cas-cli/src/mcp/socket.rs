//! Unix socket for hook-daemon communication
//!
//! Provides instant event delivery from hooks to the daemon, replacing file polling.
//!
//! # Protocol
//!
//! Events are sent as newline-delimited JSON over Unix socket.
//! The daemon listens at `.cas/daemon.sock`.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::BufReader;
use tokio::net::{UnixListener, UnixStream};

/// Events sent from hooks to the daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonEvent {
    /// Session started - register agent and PID mapping
    /// agent_name comes from CAS_AGENT_NAME in the hook's environment (Claude Code process)
    /// agent_role comes from CAS_AGENT_ROLE in the hook's environment (set by factory mode)
    SessionStart {
        session_id: String,
        agent_name: Option<String>,
        /// Agent role from CAS_AGENT_ROLE (e.g., "worker", "supervisor")
        #[serde(default)]
        agent_role: Option<String>,
        /// Claude Code's PID (the parent of the hook process)
        cc_pid: u32,
        /// Worker's clone path from CAS_CLONE_PATH (for factory mode workers)
        #[serde(default)]
        clone_path: Option<String>,
    },
    /// Session ended - clear agent cache and PID mapping
    SessionEnd {
        session_id: String,
        /// Claude Code's PID to remove from mapping
        cc_pid: Option<u32>,
    },
    /// Query session ID for a given Claude Code PID
    GetSession { cc_pid: u32 },
    /// Ping - check if daemon is alive
    Ping,
    /// Worker activity for supervisor visibility
    WorkerActivity {
        /// Session ID of the worker
        session_id: String,
        /// Event type string (maps to EventType variant)
        event_type: String,
        /// Human-readable description
        description: String,
        /// Optional entity ID (task ID, file path, etc.)
        #[serde(default)]
        entity_id: Option<String>,
    },
}

/// Response from daemon to hooks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonResponse {
    /// Acknowledgment
    Ok,
    /// Pong response to ping
    Pong,
    /// Session ID response to GetSession query
    Session { session_id: String },
    /// No session found for the given PID
    NoSession,
    /// Error
    Error { message: String },
}

/// Get the socket path for a Cassy root
pub fn socket_path(cas_root: &Path) -> PathBuf {
    cas_root.join("daemon.sock")
}

/// The advisory lock which serializes all daemon-socket ownership changes.
///
/// A Unix-socket pathname is only a directory entry.  Once a listener has
/// bound it, another process can still unlink that entry while the original
/// listener continues accepting connections.  Therefore the stale check,
/// unlink, bind, and eventual cleanup must all happen while the same lease is
/// held; otherwise two daemons can each retain a live (but differently named)
/// socket inode.
fn socket_lock_path(cas_root: &Path) -> PathBuf {
    cas_root.join("daemon.sock.lock")
}

/// The exclusive daemon-socket election lease.
///
/// The lock stays held for the listener's full lifetime.  It is deliberately
/// not dropped after `UnixListener::bind`: doing so would reopen the precise
/// check/unlink/bind race this lease closes.
#[derive(Debug)]
struct SocketLease {
    listener: UnixListener,
    lock: File,
    cas_root: PathBuf,
}

impl SocketLease {
    /// Remove this owner's pathname before releasing the election lease.
    ///
    /// No contender can bind between this unlink and `Drop`, because it must
    /// first acquire `lock`.  The listener is dropped immediately afterwards,
    /// so there is no interval where a live daemon is unreachable.
    fn cleanup(&self) {
        let _ = std::fs::remove_file(socket_path(&self.cas_root));
    }
}

impl Drop for SocketLease {
    fn drop(&mut self) {
        // Closing the descriptor would also release flock, but make the
        // happens-before boundary explicit.  The descriptor close remains the
        // fallback should unlocking fail.
        let _ = fs2::FileExt::unlock(&self.lock);
    }
}

/// Create and bind the Unix socket listener
///
/// The returned lease holds an exclusive advisory lock through the listener's
/// lifetime.  This serializes stale-socket recovery with bind and cleanup, so
/// no contender can unlink a pathname another contender has just bound.
fn create_listener(cas_root: &Path) -> std::io::Result<SocketLease> {
    use std::os::unix::net::UnixStream as StdUnixStream;

    let path = socket_path(cas_root);
    let lock_path = socket_lock_path(cas_root);
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)?;

    match fs2::FileExt::try_lock_exclusive(&lock) {
        Ok(()) => {}
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::ResourceBusy
            ) =>
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "Another daemon is already electing or listening on this socket",
            ));
        }
        Err(e) => return Err(e),
    }

    // Check if socket exists and has an active listener
    if path.exists() {
        // Try to connect - if successful, another daemon is listening
        match StdUnixStream::connect(&path) {
            Ok(_) => {
                // Another daemon is already listening - don't steal the socket
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "Another daemon is already listening on this socket",
                ));
            }
            Err(e) => {
                // Connection failed - socket is stale
                // Only remove if it's a connection refused or not found error
                if e.kind() == std::io::ErrorKind::ConnectionRefused
                    || e.kind() == std::io::ErrorKind::NotFound
                {
                    std::fs::remove_file(&path)?;
                } else {
                    // Some other error - try to remove anyway but don't fail hard
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    let listener = UnixListener::bind(&path)?;
    Ok(SocketLease {
        listener,
        lock,
        cas_root: cas_root.to_path_buf(),
    })
}

// ---------------------------------------------------------------------------
// Daemon-role election (cas-eabe / GH #163)
//
// `create_listener` deliberately refuses to steal a live socket, which keeps
// exactly one owner at a time but historically meant the losers warned once at
// startup and never tried again. Two failure modes followed from that:
//
//   1. When the owner died, no survivor ever re-bound — the project went
//      daemonless until a brand-new `cas serve` happened to start.
//   2. An owner whose on-disk binary had been deleted/replaced kept the role
//      forever, so every session silently got old-binary daemon behavior.
//
// The election loop below fixes both while preserving the bind refusal: losers
// retry on a bounded interval, and an owner that notices its own executable was
// replaced hands the role over to a current-binary survivor.
// ---------------------------------------------------------------------------

/// How often a `cas serve` that lost the `daemon.sock` bind retries it.
///
/// This is the documented upper bound on how long a project stays daemonless
/// after the socket owner dies: a survivor claims the role within one interval
/// (plus bind time) without any new process starting.
pub const ELECTION_INTERVAL: Duration = Duration::from_secs(5);

/// How often the current socket owner re-probes its own on-disk binary to see
/// whether it was deleted or replaced by an install.
pub const STALENESS_PROBE_INTERVAL: Duration = Duration::from_secs(30);

/// How long a stale owner stays off the socket after relinquishing, giving a
/// current-binary survivor time to claim the role. If nobody claims it, the
/// stale owner takes it back rather than leaving the project daemonless.
pub const STALE_HANDOVER_GRACE: Duration = Duration::from_secs(30);

/// How often the election loop checks the shutdown flag while serving.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Identity of the executable backing a running process, captured at startup.
///
/// Used to detect the "squatter" case: a `cas serve` still running from a
/// binary that has since been deleted or replaced on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExeIdentity {
    /// Executable path with any `" (deleted)"` marker stripped.
    path: PathBuf,
    /// True when the executable was already deleted when we captured it.
    deleted_at_capture: bool,
    /// `(device, inode)` of the executable when captured, if it could be read.
    file_id: Option<(u64, u64)>,
}

impl ExeIdentity {
    /// Capture the identity of the currently running executable.
    pub fn current() -> Option<Self> {
        std::env::current_exe().ok().map(Self::from_exe_path)
    }

    /// Capture the identity of a specific executable path.
    ///
    /// On Linux `/proc/self/exe` (which `current_exe` reads) resolves to
    /// `"<path> (deleted)"` once the file is unlinked, so that suffix is
    /// treated as an immediate staleness signal.
    pub fn from_exe_path(raw: PathBuf) -> Self {
        let text = raw.to_string_lossy().to_string();
        let (path, deleted_at_capture) = match text.strip_suffix(" (deleted)") {
            Some(clean) => (PathBuf::from(clean), true),
            None => (raw, false),
        };
        let file_id = file_identity(&path);
        Self {
            path,
            deleted_at_capture,
            file_id,
        }
    }

    /// The executable path this identity refers to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True when the executable behind this process is no longer the file on
    /// disk: it was already deleted at capture, has since been removed, or was
    /// replaced by a different inode (the usual shape of `cargo install`).
    pub fn is_stale(&self) -> bool {
        if self.deleted_at_capture {
            return true;
        }
        match (self.file_id, file_identity(&self.path)) {
            // Replaced in place (new inode) — an install landed under us.
            (Some(captured), Some(current)) => captured != current,
            // Removed since capture.
            (Some(_), None) => true,
            // We never had an identity to compare against; don't guess stale.
            (None, _) => false,
        }
    }

    /// Human-readable reason a doctor/status surface can print.
    pub fn staleness_reason(&self) -> Option<String> {
        if !self.is_stale() {
            return None;
        }
        let path = self.path.display();
        if self.deleted_at_capture || file_identity(&self.path).is_none() {
            Some(format!("running binary {path} no longer exists on disk"))
        } else {
            Some(format!("running binary {path} was replaced on disk"))
        }
    }
}

fn file_identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Why the socket owner gave up the daemon role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relinquish {
    /// The process is shutting down.
    Shutdown,
    /// This process's binary was deleted/replaced — hand over to a current one.
    StaleBinary,
}

/// Knobs for [`run_socket_election`]. Intervals are configurable so tests can
/// drive the same code path the daemon runs without waiting seconds.
#[derive(Debug, Clone)]
pub struct ElectionConfig {
    /// `.cas` root whose `daemon.sock` is being contested.
    pub cas_root: PathBuf,
    /// Set true when this process is shutting down; ends the election.
    pub shutdown: Arc<AtomicBool>,
    /// Mirrors whether this process currently owns the socket.
    pub owns_socket: Arc<AtomicBool>,
    /// Identity of this process's executable (None disables staleness handover).
    pub exe: Option<ExeIdentity>,
    /// Retry cadence while another process owns the socket.
    pub election_interval: Duration,
    /// Cadence of the owner's own-binary staleness probe.
    pub staleness_probe_interval: Duration,
    /// Off-socket grace after a stale owner relinquishes.
    pub stale_handover_grace: Duration,
}

impl ElectionConfig {
    /// Config with the documented production intervals.
    pub fn new(cas_root: PathBuf, shutdown: Arc<AtomicBool>) -> Self {
        Self {
            cas_root,
            shutdown,
            owns_socket: Arc::new(AtomicBool::new(false)),
            exe: ExeIdentity::current(),
            election_interval: ELECTION_INTERVAL,
            staleness_probe_interval: STALENESS_PROBE_INTERVAL,
            stale_handover_grace: STALE_HANDOVER_GRACE,
        }
    }
}

/// Own the daemon socket for as long as this process is eligible to.
///
/// Runs until the shutdown flag is set. While another process holds the socket
/// this loop retries the bind every [`ElectionConfig::election_interval`], so
/// the death of the owner is repaired by a *surviving* process rather than
/// requiring a new one. The listener's election lease serializes the stale
/// check, unlink, bind, and cleanup — so concurrent contenders resolve to
/// exactly one owner.
pub async fn run_socket_election<H, F>(config: ElectionConfig, handler: H)
where
    H: Fn(UnixStream) -> F + Clone + Send + Sync + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let mut deferral_logged = false;

    loop {
        if config.shutdown.load(Ordering::SeqCst) {
            return;
        }

        // A binary that is already stale should not claim the role at all while
        // a healthy process might; wait out the grace first.
        match create_listener(&config.cas_root) {
            Ok(lease) => {
                deferral_logged = false;
                config.owns_socket.store(true, Ordering::SeqCst);
                eprintln!(
                    "[Cassy] Daemon socket listening at {:?}",
                    socket_path(&config.cas_root)
                );

                let reason = serve_socket(&config, &lease.listener, handler.clone()).await;

                // Cleanup precedes lease release.  A successor cannot bind
                // until after the prior owner has unlinked its pathname and
                // dropped the listener, which closes the split-brain window.
                lease.cleanup();
                config.owns_socket.store(false, Ordering::SeqCst);
                drop(lease);

                match reason {
                    Relinquish::Shutdown => return,
                    Relinquish::StaleBinary => {
                        let why = config
                            .exe
                            .as_ref()
                            .and_then(ExeIdentity::staleness_reason)
                            .unwrap_or_else(|| "running binary is stale".to_string());
                        eprintln!(
                            "[Cassy] STALE DAEMON BINARY: {why}. Released {:?} for {}s so a current-binary `cas serve` can take over.",
                            socket_path(&config.cas_root),
                            config.stale_handover_grace.as_secs()
                        );
                        sleep_until_shutdown(&config.shutdown, config.stale_handover_grace).await;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if !deferral_logged {
                    eprintln!(
                        "[Cassy] Daemon socket {:?} is owned by another process; standing by (retrying every {}s to take over if it exits)",
                        socket_path(&config.cas_root),
                        config.election_interval.as_secs().max(1)
                    );
                    deferral_logged = true;
                }
                sleep_until_shutdown(&config.shutdown, config.election_interval).await;
            }
            Err(e) => {
                eprintln!("[Cassy] Warning: Could not create daemon socket: {e} (retrying)");
                sleep_until_shutdown(&config.shutdown, config.election_interval).await;
            }
        }
    }
}

/// Accept hook connections until shutdown or until our own binary goes stale.
async fn serve_socket<H, F>(
    config: &ElectionConfig,
    listener: &UnixListener,
    handler: H,
) -> Relinquish
where
    H: Fn(UnixStream) -> F + Clone + Send + Sync + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let mut staleness_probe = tokio::time::interval(config.staleness_probe_interval);
    staleness_probe.tick().await; // skip the immediate tick
    let mut shutdown_poll = tokio::time::interval(SHUTDOWN_POLL_INTERVAL);
    shutdown_poll.tick().await;

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let handler = handler.clone();
                        tokio::spawn(async move { handler(stream).await });
                    }
                    Err(e) => {
                        eprintln!("[Cassy] Socket accept error: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
            _ = staleness_probe.tick() => {
                if config.exe.as_ref().is_some_and(ExeIdentity::is_stale) {
                    return Relinquish::StaleBinary;
                }
            }
            _ = shutdown_poll.tick() => {
                if config.shutdown.load(Ordering::SeqCst) {
                    return Relinquish::Shutdown;
                }
            }
        }
    }
}

/// Sleep, waking early if shutdown is requested.
async fn sleep_until_shutdown(shutdown: &Arc<AtomicBool>, total: Duration) {
    let step = SHUTDOWN_POLL_INTERVAL.min(total);
    let mut slept = Duration::ZERO;
    while slept < total {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(step).await;
        slept += step;
    }
}

/// Send an event to the daemon (called by hooks)
///
/// This is a synchronous blocking call suitable for hook context.
pub fn send_event(cas_root: &Path, event: &DaemonEvent) -> std::io::Result<DaemonResponse> {
    use std::io::{BufRead, Write};
    use std::os::unix::net::UnixStream as StdUnixStream;

    let path = socket_path(cas_root);
    let mut stream = StdUnixStream::connect(&path)?;

    // Set timeout for hook context (don't block too long)
    stream.set_read_timeout(Some(std::time::Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_millis(500)))?;

    // Send event as JSON line
    let json = serde_json::to_string(event)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    writeln!(stream, "{json}")?;
    stream.flush()?;

    // Read response
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    serde_json::from_str(&line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// Read event from socket connection (async, called by daemon)
pub async fn read_event(stream: &mut UnixStream) -> Option<DaemonEvent> {
    use tokio::io::AsyncBufReadExt;

    let mut reader = BufReader::new(&mut *stream);
    let mut line = String::new();

    match reader.read_line(&mut line).await {
        Ok(0) => None, // EOF
        Ok(_) => match serde_json::from_str::<DaemonEvent>(&line) {
            Ok(event) => Some(event),
            Err(e) => {
                eprintln!("[Cassy] Invalid event from hook: {e}");
                None
            }
        },
        Err(e) => {
            eprintln!("[Cassy] Error reading from hook socket: {e}");
            None
        }
    }
}

/// Send response back to hook (async)
pub async fn send_response(
    stream: &mut UnixStream,
    response: &DaemonResponse,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let json = serde_json::to_string(response)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    stream.write_all(json.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Election config wired for tests: sub-second intervals, no exe staleness.
    fn test_config(root: &Path) -> ElectionConfig {
        ElectionConfig {
            cas_root: root.to_path_buf(),
            shutdown: Arc::new(AtomicBool::new(false)),
            owns_socket: Arc::new(AtomicBool::new(false)),
            exe: None,
            election_interval: Duration::from_millis(50),
            staleness_probe_interval: Duration::from_millis(50),
            stale_handover_grace: Duration::from_millis(50),
        }
    }

    /// A handler that answers every event with `Pong`.
    fn pong_handler()
    -> impl Fn(UnixStream) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    + Clone
    + Send
    + Sync
    + 'static {
        |mut stream: UnixStream| {
            Box::pin(async move {
                if read_event(&mut stream).await.is_some() {
                    let _ = send_response(&mut stream, &DaemonResponse::Pong).await;
                }
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }
    }

    async fn wait_for(flag: &Arc<AtomicBool>, want: bool, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if flag.load(Ordering::SeqCst) == want {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        flag.load(Ordering::SeqCst) == want
    }

    #[tokio::test]
    async fn create_listener_refuses_a_live_socket() {
        let dir = tempfile::tempdir().unwrap();
        let _owner = create_listener(dir.path()).expect("first bind wins");

        let err = create_listener(dir.path()).expect_err("second bind must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    }

    #[tokio::test]
    async fn create_listener_reclaims_a_stale_socket_file() {
        let dir = tempfile::tempdir().unwrap();
        let owner = create_listener(dir.path()).unwrap();
        // Owner "dies" without cleanup: the socket file survives but nothing
        // is listening on it.
        drop(owner);
        assert!(socket_path(dir.path()).exists());

        let _survivor = create_listener(dir.path()).expect("stale socket file must be reclaimable");
    }

    #[test]
    fn exe_identity_flags_deleted_and_replaced_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("cas");
        std::fs::write(&exe, b"v1").unwrap();

        let identity = ExeIdentity::from_exe_path(exe.clone());
        assert!(!identity.is_stale(), "untouched binary is not stale");
        assert!(identity.staleness_reason().is_none());

        // Replaced in place with a different inode (what an install does).
        let replacement = dir.path().join("cas.new");
        std::fs::write(&replacement, b"v2").unwrap();
        std::fs::rename(&replacement, &exe).unwrap();
        assert!(identity.is_stale(), "replaced binary must read as stale");
        assert!(
            identity
                .staleness_reason()
                .expect("reason")
                .contains("replaced")
        );

        // Deleted outright.
        std::fs::remove_file(&exe).unwrap();
        assert!(identity.is_stale());
        assert!(
            identity
                .staleness_reason()
                .expect("reason")
                .contains("no longer exists")
        );
    }

    #[test]
    fn exe_identity_treats_deleted_marker_as_stale() {
        // Linux renders /proc/self/exe for an unlinked binary as "<path> (deleted)".
        let identity = ExeIdentity::from_exe_path(PathBuf::from("/usr/local/bin/cas (deleted)"));
        assert_eq!(identity.path(), Path::new("/usr/local/bin/cas"));
        assert!(identity.is_stale());
    }

    /// AC(1): when the owner dies, an ALREADY-RUNNING survivor claims the role
    /// within a bounded interval — no new process starts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn survivor_claims_the_role_after_the_owner_dies() {
        let dir = tempfile::tempdir().unwrap();
        let owner = create_listener(dir.path()).expect("owner binds first");

        let config = test_config(dir.path());
        let owns = Arc::clone(&config.owns_socket);
        let shutdown = Arc::clone(&config.shutdown);
        let interval = config.election_interval;
        tokio::spawn(run_socket_election(config, pong_handler()));

        // The survivor must defer while the owner is alive.
        tokio::time::sleep(interval * 4).await;
        assert!(
            !owns.load(Ordering::SeqCst),
            "survivor must not steal a live socket"
        );

        // Owner dies, leaving its socket file behind.
        drop(owner);

        assert!(
            wait_for(&owns, true, Duration::from_secs(5)).await,
            "survivor must claim the vacant role without a new process starting"
        );

        // ...and it actually serves hook traffic.
        let root = dir.path().to_path_buf();
        let response = tokio::task::spawn_blocking(move || send_event(&root, &DaemonEvent::Ping))
            .await
            .unwrap()
            .expect("ping the new owner");
        assert!(matches!(response, DaemonResponse::Pong));

        shutdown.store(true, Ordering::SeqCst);
    }

    /// AC(2): an owner whose binary was replaced hands the role to a
    /// current-binary survivor.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stale_binary_owner_hands_the_role_over() {
        let dir = tempfile::tempdir().unwrap();
        let exe_dir = tempfile::tempdir().unwrap();
        let exe = exe_dir.path().join("cas");
        std::fs::write(&exe, b"old").unwrap();
        let stale_identity = ExeIdentity::from_exe_path(exe.clone());
        std::fs::remove_file(&exe).unwrap();
        assert!(stale_identity.is_stale());

        // Stale owner: slow probe so we can observe it holding the role first,
        // and a long grace so it cannot race the healthy survivor to re-claim.
        let mut stale = test_config(dir.path());
        stale.exe = Some(stale_identity);
        stale.staleness_probe_interval = Duration::from_millis(400);
        stale.stale_handover_grace = Duration::from_secs(60);
        let stale_owns = Arc::clone(&stale.owns_socket);
        let stale_shutdown = Arc::clone(&stale.shutdown);
        tokio::spawn(run_socket_election(stale, pong_handler()));

        assert!(
            wait_for(&stale_owns, true, Duration::from_secs(5)).await,
            "stale process should take a vacant socket rather than leave it empty"
        );

        // A healthy current-binary serve is standing by.
        let healthy = test_config(dir.path());
        let healthy_owns = Arc::clone(&healthy.owns_socket);
        let healthy_shutdown = Arc::clone(&healthy.shutdown);
        tokio::spawn(run_socket_election(healthy, pong_handler()));

        assert!(
            wait_for(&stale_owns, false, Duration::from_secs(5)).await,
            "stale owner must relinquish the role"
        );
        assert!(
            wait_for(&healthy_owns, true, Duration::from_secs(5)).await,
            "current-binary survivor must take the role over"
        );

        let root = dir.path().to_path_buf();
        let response = tokio::task::spawn_blocking(move || send_event(&root, &DaemonEvent::Ping))
            .await
            .unwrap()
            .expect("ping the new owner");
        assert!(matches!(response, DaemonResponse::Pong));

        stale_shutdown.store(true, Ordering::SeqCst);
        healthy_shutdown.store(true, Ordering::SeqCst);
    }

    /// AC(3): concurrent contenders resolve to exactly one owner.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_election_resolves_to_exactly_one_owner() {
        use tokio::sync::Barrier;

        // The default is one deterministic election round for the normal
        // suite.  The same test can be stressed in CI/triage without a retry
        // loop by setting `CAS_SOCKET_ELECTION_STRESS_ITERATIONS`; each round
        // still releases five contenders on the same barrier.
        let rounds = std::env::var("CAS_SOCKET_ELECTION_STRESS_ITERATIONS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|rounds| *rounds > 0)
            .unwrap_or(1);

        for round in 0..rounds {
            let dir = tempfile::tempdir().unwrap();
            let mut flags = Vec::new();
            let mut shutdowns = Vec::new();
            let mut contenders = Vec::new();
            // Do not measure whether Tokio happened to schedule every task inside
            // an arbitrary wall-clock window. Releasing all contenders together
            // makes this an actual election test, even on an oversubscribed CI
            // runner where a fixed 600ms sleep can observe zero owners.
            let start = Arc::new(Barrier::new(6));

            for _ in 0..5 {
                let config = test_config(dir.path());
                flags.push(Arc::clone(&config.owns_socket));
                shutdowns.push(Arc::clone(&config.shutdown));
                let start = Arc::clone(&start);
                contenders.push(tokio::spawn(async move {
                    start.wait().await;
                    run_socket_election(config, pong_handler()).await;
                }));
            }

            start.wait().await;
            assert!(
                wait_for_any_owner(&flags, Duration::from_secs(5)).await,
                "one concurrent contender must claim daemon.sock"
            );
            // Observe a full scheduling turn after the winner publishes
            // ownership.  The assertion remains exactly-one and now catches a
            // contender that unlinks and rebinds after the first observation.
            for _ in 0..10 {
                let owners = flags.iter().filter(|f| f.load(Ordering::SeqCst)).count();
                assert_eq!(
                    owners, 1,
                    "exactly one contender may own daemon.sock (round {round})"
                );
                tokio::task::yield_now().await;
            }

            for shutdown in shutdowns {
                shutdown.store(true, Ordering::SeqCst);
            }
            for contender in contenders {
                tokio::time::timeout(Duration::from_secs(2), contender)
                    .await
                    .expect("contender must stop after shutdown")
                    .expect("contender task must not panic");
            }
        }
    }

    async fn wait_for_any_owner(flags: &[Arc<AtomicBool>], timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if flags.iter().any(|flag| flag.load(Ordering::SeqCst)) {
                return true;
            }
            tokio::task::yield_now().await;
        }
        flags.iter().any(|flag| flag.load(Ordering::SeqCst))
    }
}
