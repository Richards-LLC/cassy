//! Factory daemon - owns PTYs and runs TUI with PTY forwarding
//!
//! The daemon:
//! - Owns all PTY file descriptors for agents
//! - Runs the full TUI event loop
//! - Renders to a buffer and sends to connected clients
//! - Receives input from clients and processes it
//! - Persists across TUI attach/detach cycles

use crate::ui::factory::app::{FactoryApp, FactoryConfig, WorkerSpawnResult};
use crate::ui::factory::buffer_backend::BufferBackend;
use crate::ui::factory::session::SessionManager;
use ratatui::Terminal;
use std::collections::{HashMap, VecDeque};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use tokio::task::JoinHandle;

/// Special escape sequence prefix for control messages from client
/// Using OSC (Operating System Command) private sequence: ESC ] 777 ;
const CONTROL_PREFIX: &[u8] = b"\x1b]777;";
const CONTROL_SUFFIX: u8 = 0x07; // BEL

/// Max buffered output per client before forcing a full redraw.
const MAX_CLIENT_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
/// Seconds before another client can take input ownership.
const INPUT_OWNER_IDLE_SECS: u64 = 2;

/// Factory daemon configuration
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Session name
    pub session_name: String,
    /// Factory configuration
    pub factory_config: FactoryConfig,
    /// Run in foreground (don't daemonize)
    pub foreground: bool,
    /// Enable boot progress socket handshake before entering daemon loop.
    pub boot_progress: bool,
    /// Enable cloud phone-home (push state/events to CAS Cloud)
    pub phone_home: bool,
}

/// Client view mode determines rendering layout
#[derive(Debug, Clone, Copy, PartialEq)]
enum ClientViewMode {
    /// Full factory TUI (desktop)
    Full,
    /// Compact supervisor-focused view (phone/narrow terminal)
    Compact,
}

/// Threshold below which a client is auto-assigned compact mode
const COMPACT_WIDTH_THRESHOLD: u16 = 80;

/// A connected TUI client
struct ClientConnection {
    /// Socket stream
    stream: UnixStream,
    /// Input buffer for parsing control sequences
    input_buf: Vec<u8>,
    /// Buffered output for this client
    output_buf: VecDeque<u8>,
    /// Whether this client needs a full redraw
    needs_full_redraw: bool,
    /// Client's view mode
    view_mode: ClientViewMode,
    /// Client's terminal dimensions
    client_cols: u16,
    client_rows: u16,
}

/// A connected GUI client (cas-desktop) using length-prefixed JSON protocol
struct GuiConnection {
    /// Socket stream
    stream: UnixStream,
    /// Buffer for accumulating incoming bytes until a complete message is available
    read_buf: Vec<u8>,
    /// Buffered outgoing framed messages
    write_buf: VecDeque<u8>,
    /// Per-pane dimensions reported by this client (pane_id -> (cols, rows))
    pane_sizes: HashMap<String, (u16, u16)>,
}

/// A connected WebSocket client using tokio-tungstenite
struct WsConnection {
    /// Sink half of the split WebSocketStream (for sending messages)
    sink: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        tokio_tungstenite::tungstenite::Message,
    >,
    /// Stream half of the split WebSocketStream (for receiving messages)
    stream: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    >,
    /// Per-pane dimensions reported by this client (pane_id -> (cols, rows))
    pane_sizes: HashMap<String, (u16, u16)>,
}

/// A single pending spawn action (one worker at a time)
#[derive(Debug)]
enum PendingSpawn {
    /// Spawn a worker with an auto-generated name
    Anonymous {
        isolate: bool,
        spec: Option<cas_mux::WorkerSpec>,
        /// cas-6913: task to pre-assign once this worker finishes spawning.
        /// Only ever `Some` for single-worker requests — validated at the
        /// MCP layer (`factory_spawn_workers`) before enqueueing.
        task_id: Option<String>,
    },
    /// Spawn a worker with a specific name
    Named {
        name: String,
        isolate: bool,
        spec: Option<cas_mux::WorkerSpec>,
        /// cas-6913: task to pre-assign once this worker finishes spawning.
        task_id: Option<String>,
    },
    /// Shutdown workers
    Shutdown {
        count: Option<usize>,
        names: Vec<String>,
        force: bool,
    },
    /// Respawn a crashed worker
    Respawn(String),
    /// Spawn a shell pane
    Shell { name: String, shell: Option<String> },
    /// Kill a shell pane
    KillShell { name: String },
}

/// Factory daemon state
pub struct FactoryDaemon {
    /// Session name
    session_name: String,
    /// The factory app (owns the mux)
    app: FactoryApp,
    /// Socket listener
    listener: UnixListener,
    /// Connected clients
    clients: HashMap<usize, ClientConnection>,
    /// Next client ID
    next_client_id: usize,
    /// Client id that owns input/resize focus
    owner_client_id: Option<usize>,
    /// Last time the owner sent input
    owner_last_activity: Instant,
    /// Session manager
    session_manager: SessionManager,
    /// Shutdown flag
    shutdown: Arc<AtomicBool>,
    /// Terminal size (full mode)
    cols: u16,
    rows: u16,
    /// Resize debounce: pending resize event and when it was received
    pending_resize: Option<(u16, u16)>,
    pending_resize_at: Instant,
    /// Compact mode terminal (separate render target for compact clients)
    compact_terminal: Option<Terminal<BufferBackend>>,
    compact_cols: u16,
    compact_rows: u16,
    /// Pending spawn/shutdown actions (processed one per tick to avoid blocking TUI)
    pending_spawns: VecDeque<PendingSpawn>,
    /// In-flight background spawn task: (worker_name, per-spawn spec override, task_id to
    /// pre-assign (cas-6913), join_handle).
    /// One at a time, runs git worktree ops off main thread.
    /// The spec/task_id carry the caller-supplied overrides through the async gap to
    /// finish_worker_spawn.
    spawn_task: Option<(
        String,
        Option<cas_mux::WorkerSpec>,
        Option<String>,
        JoinHandle<anyhow::Result<WorkerSpawnResult>>,
    )>,
    /// Cloud phone-home WebSocket client handle
    cloud_handle: Option<cloud_client::CloudClientHandle>,
    /// Whether cloud phone-home should be started (deferred from init for fork-first path)
    phone_home: bool,
    /// Remote relay clients connected via cloud WebSocket
    relay_clients: HashMap<String, runtime::relay::RelayClient>,
    /// Per-pane web watchers: pane_name -> set of watcher IDs
    pane_watchers: HashMap<String, std::collections::HashSet<String>>,
    /// Per-pane ring buffer of raw PTY bytes for replay on attach
    pane_buffers: HashMap<String, runtime::relay::PaneBuffer>,
    /// GUI socket listener (for desktop GUI clients using JSON protocol)
    gui_listener: UnixListener,
    /// Connected GUI clients
    gui_clients: HashMap<usize, GuiConnection>,
    /// Next GUI client ID
    next_gui_client_id: usize,
    /// WebSocket listener for network clients
    ws_listener: Option<tokio::net::TcpListener>,
    /// Connected WebSocket clients
    ws_clients: HashMap<usize, WsConnection>,
    /// Next WebSocket client ID
    next_ws_client_id: usize,
    /// Per-pane sizes allocated by TUI layout (pane_id -> (cols, rows))
    tui_pane_sizes: HashMap<String, (u16, u16)>,
    /// Per-pane sizes reported by web viewers (pane_id -> (cols, rows))
    web_pane_sizes: HashMap<String, (u16, u16)>,
    /// Native Agent Teams manager for inter-agent messaging.
    /// When present, messages are delivered via Teams inbox files instead of PTY injection.
    teams: Option<runtime::teams::TeamsManager>,
    /// Notification socket for instant prompt queue wakeup.
    /// Falls back to pure polling if socket creation fails.
    notify_rx: Option<cas_factory::DaemonNotifier>,
    /// Workers that have been shut down or crashed — their queued messages are dropped.
    dead_workers: std::collections::HashSet<String>,
    /// In-flight spawns cancelled by a shutdown that targeted that specific
    /// generation. Entries are consumed when the matching spawn task finishes.
    cancelled_spawns: std::collections::HashSet<String>,
    /// Tracks last idle-like message time per worker source for dedup.
    /// Prevents idle spam when workers send repeated "standing by" / "ready" messages.
    last_idle_message_times: HashMap<String, std::time::Instant>,
    /// Last bounded sweep that terminalized aged prompts for targets no
    /// longer belonging to this factory session.
    last_prompt_poison_sweep: Option<std::time::Instant>,
    /// Epic IDs already logged as "resuming" (prevents log spam every refresh cycle)
    resumed_epic_ids: std::collections::HashSet<String>,
}

/// Parsed control events from client
#[derive(Debug)]
enum ControlEvent {
    Resize(u16, u16),
    SetMode(ClientViewMode),
    MouseScrollUp,
    MouseScrollDown,
    MouseClick { col: u16, row: u16 },
    DropImage { col: u16, row: u16, path: String },
    /// Bracketed paste from the client. The client coalesces a paste into one
    /// event (stripping the terminal's \x1b[200~/\x1b[201~ markers), so we carry
    /// the literal payload here and re-wrap it as bracketed paste when injecting
    /// into the focused pane — otherwise embedded newlines reach the inner CLI as
    /// bare Enter keys and submit the prompt mid-paste (cas-5702).
    Paste { payload: String },
    SetSelectMode(bool),
}

pub mod cloud_client;
mod fork_first;
mod imports;
mod process;
pub(crate) mod runtime;

pub use fork_first::{DaemonInitPhase, ForkFirstResult, fork_first_daemon};
pub use process::{
    ForkResult, daemonize, fork_into_daemon, run_daemon, run_daemon_after_fork,
    run_daemon_with_boot_progress,
};

#[cfg(test)]
mod tests {
    use super::PendingSpawn;
    use cas_mux::{Effort, SupervisorCli, WorkerSpec};

    /// AC3 (cas-4cae): verify a codex spec stored in PendingSpawn reaches mux.add_worker.
    ///
    /// Simulates the routing chain introduced by T2:
    ///   ClientMessage::SpawnWorkers { specs: [Some(codex_spec)] }
    ///     → ws_client/gui_client handler → PendingSpawn::Named { spec: Some(..) }
    ///     → queue_and_events: spec extracted from spawn_task tuple
    ///     → finish_worker_spawn(result, teams, pending_spec)
    ///     → mux.add_worker(..., spec)                   ← tested via build_add_worker_config
    ///
    /// build_add_worker_config is the PTY-free test proxy for add_worker; it builds
    /// the PtyConfig that add_worker would spawn without touching real processes.
    #[test]
    fn pending_spawn_codex_spec_routes_to_mux_add_worker() {
        let codex_spec = WorkerSpec {
            name: Some("alice".to_string()),
            cli: SupervisorCli::Codex,
            model: Some("gpt-5o".to_string()),
            effort: Some(Effort::Medium),
        };

        // Step 1: handler pushes PendingSpawn with the spec (simulates ws_client / gui_client).
        let pending = PendingSpawn::Named {
            name: "alice".to_string(),
            isolate: false,
            spec: Some(codex_spec.clone()),
            task_id: None,
        };

        // Step 2: queue_and_events extracts spec from spawn_task tuple after the blocking
        // prepare phase completes.  Verify PendingSpawn correctly preserves the spec.
        let extracted_spec = match pending {
            PendingSpawn::Named { spec, .. } => spec,
            _ => panic!("unexpected PendingSpawn variant"),
        };
        assert_eq!(
            extracted_spec,
            Some(codex_spec.clone()),
            "PendingSpawn::Named must preserve the caller-supplied WorkerSpec"
        );

        // Step 3: spec is forwarded to mux.add_worker (here via build_add_worker_config,
        // the PTY-free proxy used in tests).  Verify codex spec produces codex binary.
        let mut mux = cas_mux::Mux::new(24, 80);
        // Default is Claude — the spec override must win.
        mux.set_default_worker_spec(WorkerSpec {
            name: None,
            cli: SupervisorCli::Claude,
            model: None,
            effort: None,
        });
        let tmpdir = tempfile::TempDir::new().unwrap();
        let config = mux
            .build_add_worker_config("alice", tmpdir.path().to_path_buf(), None, "supervisor", None, extracted_spec);

        // When `nice` wraps the binary, the actual command is at args[2].
        // See cas-mux/src/mux_tests/tests.rs::effective_command for the same pattern.
        let effective_cmd = if config.command == "nice" {
            config.args.get(2).map(String::as_str).unwrap_or("nice")
        } else {
            &config.command
        };
        assert!(
            effective_cmd.contains("codex"),
            "codex WorkerSpec must reach mux.add_worker and select the codex binary; \
             got effective command: {:?} (command={:?}, args={:?})",
            effective_cmd,
            config.command,
            config.args
        );
    }

    /// cas-6913: verify a task_id survives the same PendingSpawn → spawn_task
    /// tuple routing chain as the WorkerSpec above, so `finish_worker_spawn`
    /// (which extracts it from the tuple, see queue_and_events.rs) actually
    /// receives it and can pre-assign the task once the worker is up.
    #[test]
    fn pending_spawn_task_id_survives_named_and_anonymous_variants() {
        let named = PendingSpawn::Named {
            name: "alice".to_string(),
            isolate: false,
            spec: None,
            task_id: Some("cas-abc1".to_string()),
        };
        let extracted = match named {
            PendingSpawn::Named { task_id, .. } => task_id,
            _ => panic!("unexpected PendingSpawn variant"),
        };
        assert_eq!(
            extracted,
            Some("cas-abc1".to_string()),
            "PendingSpawn::Named must preserve the caller-supplied task_id"
        );

        let anonymous = PendingSpawn::Anonymous {
            isolate: true,
            spec: None,
            task_id: Some("cas-xyz9".to_string()),
        };
        let extracted = match anonymous {
            PendingSpawn::Anonymous { task_id, .. } => task_id,
            _ => panic!("unexpected PendingSpawn variant"),
        };
        assert_eq!(
            extracted,
            Some("cas-xyz9".to_string()),
            "PendingSpawn::Anonymous must preserve the caller-supplied task_id"
        );
    }
}
