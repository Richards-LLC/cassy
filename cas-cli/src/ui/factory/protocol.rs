//! Socket protocol for factory daemon communication
//!
//! Defines the message types exchanged between the factory daemon (which owns PTYs)
//! and the TUI client (which renders and sends input).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Original daemon protocol version used before explicit negotiation existed.
pub const LEGACY_PROTOCOL_VERSION: u32 = 1;
/// Current additive daemon protocol version.
pub const PROTOCOL_VERSION: u32 = 3;
/// Maximum recent PTY output replayed per active pane on client attach.
pub(crate) const COMMANDER_REPLAY_BYTES_PER_PANE: usize = 64 * 1024;

/// Independently negotiable daemon protocol features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolCapability {
    TargetedInterrupt,
    AttributedSendMessage,
    /// A pane can be initialized from terminal state instead of a raw-byte tail.
    AuthoritativePaneKeyframes,
    /// Historical rows can be requested independently from the live viewport.
    PagedScrollback,
}

pub fn daemon_capabilities() -> Vec<ProtocolCapability> {
    vec![
        ProtocolCapability::TargetedInterrupt,
        ProtocolCapability::AttributedSendMessage,
        ProtocolCapability::AuthoritativePaneKeyframes,
        ProtocolCapability::PagedScrollback,
    ]
}

fn legacy_protocol_version() -> u32 {
    LEGACY_PROTOCOL_VERSION
}

/// Attribution supplied by the authenticated hub boundary for semantic messages.
///
/// H3 transports and persists these values but does not authenticate them; H2 owns
/// that boundary. Every field is required on the wire. `null` means explicitly
/// unavailable and is never interpreted as supervisor or MCP identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAttribution {
    pub device_id: Option<String>,
    pub credential_id: Option<String>,
    pub device_label: Option<String>,
    pub operator_label: Option<String>,
    pub controller_origin: Option<String>,
    pub request_id: Option<String>,
}

impl MessageAttribution {
    /// Durable prompt-queue sender label. The fixed `commander:` namespace
    /// prevents a remote label from impersonating `supervisor` or `mcp`.
    pub fn queue_source(&self) -> String {
        fn component(value: Option<&str>, fallback: &str) -> String {
            let cleaned: String = value
                .unwrap_or(fallback)
                .trim()
                .chars()
                .filter(|ch| !ch.is_control())
                .take(80)
                .collect();
            if cleaned.is_empty() {
                fallback.to_string()
            } else {
                cleaned
            }
        }

        let operator = component(self.operator_label.as_deref(), "unknown-operator");
        let device = component(
            self.device_label.as_deref().or(self.device_id.as_deref()),
            "unknown-device",
        );
        format!("commander:{operator}@{device}")
    }
}

/// Messages sent from TUI client to daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Attach to the session (sent on connect)
    Attach {
        /// Request full scrollback buffer
        request_scrollback: bool,
    },

    /// Request a fresh, authoritative terminal-state keyframe for one pane.
    RequestPaneKeyframe { pane_id: String },

    /// Request a bounded page from the pane's historical screen rows.
    ScrollbackRequest {
        pane_id: String,
        generation: u64,
        start_row: u32,
        count: u16,
    },

    /// Detach from the session (graceful disconnect)
    Detach,

    /// Send keyboard input to a specific pane
    Input {
        /// Target pane ID
        pane_id: String,
        /// Raw bytes to send
        data: Vec<u8>,
    },

    /// Send keyboard input to the focused pane
    InputFocused {
        /// Raw bytes to send
        data: Vec<u8>,
    },

    /// Change focus to a specific pane
    Focus {
        /// Target pane ID
        pane_id: String,
    },

    /// Focus next pane
    FocusNext,

    /// Focus previous pane
    FocusPrev,

    /// Request terminal resize (global, used by TUI clients)
    Resize {
        /// New column count
        cols: u16,
        /// New row count
        rows: u16,
    },

    /// Resize a specific pane (used by GUI clients where each pane has its own terminal)
    ResizePane {
        /// Target pane ID
        pane_id: String,
        /// New column count
        cols: u16,
        /// New row count
        rows: u16,
    },

    /// Spawn new workers
    SpawnWorkers {
        /// Number of workers to spawn
        count: usize,
        /// Optional specific names
        names: Vec<String>,
        /// Per-worker spec overrides, parallel to `names`.
        /// Empty or shorter than `names` means use session defaults for the unspecified slots.
        /// `None` at index i means use the session default for that worker.
        /// Old clients that omit this field get an empty vec (backwards-compatible).
        #[serde(default)]
        specs: Vec<Option<cas_mux::WorkerSpec>>,
    },

    /// Shutdown workers
    ShutdownWorkers {
        /// Number to shutdown (0 = all)
        count: usize,
        /// Optional specific names (overrides count)
        names: Vec<String>,
    },

    /// Inject a prompt into a pane
    Inject {
        /// Target pane ID
        pane_id: String,
        /// Prompt text to inject
        prompt: String,
        /// Urgent (interrupt-and-redirect) delivery (cas-c931): when true,
        /// break the target's in-flight turn (Esc) and inject via the PTY,
        /// bypassing the Claude Code inbox even in agent-teams mode. Defaults
        /// to false (normal inbox/queue delivery) for older clients that omit
        /// the field.
        #[serde(default)]
        urgent: bool,
    },

    /// Break one pane's current harness turn by name. This is additive and
    /// deliberately distinct from legacy focused-pane Ctrl+C `Interrupt`.
    InterruptPane { pane_id: String },

    /// Enqueue an attributed semantic message through the durable coordination
    /// delivery path (inbox + wake / urgent interrupt-and-redirect machinery).
    SendMessage {
        target: String,
        text: String,
        summary: Option<String>,
        #[serde(default)]
        urgent: bool,
        attribution: MessageAttribution,
    },

    /// Request current state snapshot
    GetState,

    /// Ping to check connection
    Ping,

    /// Interrupt the focused pane (Ctrl+C)
    Interrupt,

    /// Spawn a new shell pane
    SpawnShell {
        /// Pane name
        name: String,
        /// Shell command (uses $SHELL if not specified)
        shell: Option<String>,
    },

    /// Kill a shell pane
    KillShell {
        /// Pane name to kill
        name: String,
    },
}

/// Who owns the PTY geometry of a pane (cas-37f8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneSizeAuthority {
    /// The operator's local dashboard is attached and owns the size; viewers
    /// render it and must not try to drive the PTY.
    LocalDashboard,
    /// No local dashboard is attached, so the smallest attached viewer owns
    /// the size.
    Viewer,
}

/// Messages sent from daemon to TUI client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonMessage {
    /// Welcome message on attach with current state
    Welcome {
        /// Session name
        session_name: String,
        /// Current state snapshot
        state: SessionState,
        /// Scrollback buffers for each pane (if requested)
        scrollback: Option<HashMap<String, Vec<Vec<u8>>>>,
        /// Additive version negotiation. Missing means the legacy protocol.
        #[serde(default = "legacy_protocol_version")]
        protocol_version: u32,
        /// Independently negotiable features. Missing means no new controls.
        #[serde(default)]
        capabilities: Vec<ProtocolCapability>,
        /// Content-free attach metadata used by protocol v3 clients.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pane_bootstrap: Vec<PaneBootstrap>,
    },

    /// An authoritative ANSI serialization of the pane's current terminal state.
    PaneKeyframe {
        pane_id: String,
        epoch: u64,
        seq: u64,
        cols: u16,
        rows: u16,
        ansi: Vec<u8>,
    },

    /// The authoritative PTY geometry of a pane, and who owns it.
    ///
    /// Sent to a client whose `ResizePane` was not applied verbatim — most
    /// importantly when the operator's local dashboard owns the geometry and a
    /// remote viewer asked for something smaller. The viewer must render this
    /// size (scale / letterbox / scroll) instead of retrying the resize
    /// (cas-37f8).
    PaneSize {
        pane_id: String,
        cols: u16,
        rows: u16,
        authority: PaneSizeAuthority,
    },

    /// A bounded, styled page of historical screen rows.
    ScrollbackPage {
        pane_id: String,
        generation: u64,
        start_row: u32,
        next_row: Option<u32>,
        rows: Vec<cas_factory_protocol::CacheRow>,
    },

    /// Terminal output from a pane
    Output {
        /// Source pane ID
        pane_id: String,
        /// Output data (terminal escape sequences included)
        data: Vec<u8>,
    },

    /// Best-effort, server-computed description of the current session.
    /// All observers receive the same value; terminal content is never sent.
    SessionSummary { summary: SessionCardSummary },

    /// A pane exited
    PaneExited {
        /// Pane ID that exited
        pane_id: String,
        /// Exit code if available
        exit_code: Option<i32>,
    },

    /// A pane was added
    PaneAdded {
        /// New pane info
        pane: PaneInfo,
    },

    /// A pane was removed
    PaneRemoved {
        /// Removed pane ID
        pane_id: String,
    },

    /// Focus changed
    FocusChanged {
        /// Previously focused pane (if any)
        from: Option<String>,
        /// Newly focused pane
        to: String,
    },

    /// State update (periodic or on significant change)
    StateUpdate {
        /// Updated state
        state: SessionState,
    },

    /// Error response
    Error {
        /// Error message
        message: String,
    },

    /// Pong response to ping
    Pong,

    /// Acknowledgment of detach
    Detached,

    /// Initialization progress (sent during daemon startup)
    InitProgress {
        /// Current step name
        step: String,
        /// Step number (1-based)
        step_num: u8,
        /// Total steps
        total_steps: u8,
        /// Whether this step completed successfully
        completed: bool,
    },

    /// Agent spawn progress
    AgentProgress {
        /// Agent name
        name: String,
        /// Whether this is a supervisor (vs worker)
        is_supervisor: bool,
        /// Progress 0.0-1.0
        progress: f32,
        /// Whether spawn completed
        ready: bool,
    },

    /// Initialization complete - daemon ready for TUI
    InitComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCardSummary {
    pub title: String,
    pub description: String,
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_on: Option<String>,
    pub generated_at: String,
}

/// Snapshot of session state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Currently focused pane ID
    pub focused_pane: Option<String>,
    /// All panes in the session
    pub panes: Vec<PaneInfo>,
    /// Current epic ID (if any)
    pub epic_id: Option<String>,
    /// Current epic title (if any)
    pub epic_title: Option<String>,
    /// Terminal dimensions
    pub cols: u16,
    pub rows: u16,
}

/// Information about a single pane
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneInfo {
    /// Pane ID (also the name)
    pub id: String,
    /// Pane kind
    pub kind: PaneKind,
    /// Whether this pane is focused
    pub focused: bool,
    /// Title for display
    pub title: String,
    /// Whether the pane process has exited
    pub exited: bool,
}

/// Per-pane attach metadata. This deliberately contains no terminal content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneBootstrap {
    pub pane_id: String,
    pub epoch: u64,
    pub cols: u16,
    pub rows: u16,
    pub scrollback_start_row: u32,
    pub scrollback_end_row: u32,
}

/// Kind of pane (matches cas_mux::PaneKind)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneKind {
    /// Worker agent pane
    Worker,
    /// Supervisor agent pane
    Supervisor,
    /// Director panel (no PTY)
    Director,
    /// Generic shell
    Shell,
}

impl From<cas_mux::PaneKind> for PaneKind {
    fn from(kind: cas_mux::PaneKind) -> Self {
        match kind {
            cas_mux::PaneKind::Worker => PaneKind::Worker,
            cas_mux::PaneKind::Supervisor => PaneKind::Supervisor,
            cas_mux::PaneKind::Director => PaneKind::Director,
            cas_mux::PaneKind::Shell => PaneKind::Shell,
        }
    }
}

/// Session metadata persisted to disk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Session name
    pub name: String,
    /// When the session was created
    pub created_at: String,
    /// Daemon process ID
    pub daemon_pid: u32,
    /// OS process-start fingerprint paired with `daemon_pid` to reject reuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_pid_starttime: Option<u64>,
    /// Socket path
    pub socket_path: String,
    /// WebSocket server port (for client connections)
    #[serde(default)]
    pub ws_port: Option<u16>,
    /// Log directory for this session
    #[serde(default)]
    pub log_dir: Option<String>,
    /// Daemon stderr log path
    #[serde(default)]
    pub daemon_log_path: Option<String>,
    /// Daemon tracing log path
    #[serde(default)]
    pub daemon_trace_log_path: Option<String>,
    /// Server stderr log path
    #[serde(default)]
    pub server_log_path: Option<String>,
    /// Server tracing log path
    #[serde(default)]
    pub server_trace_log_path: Option<String>,
    /// TUI tracing log path
    #[serde(default)]
    pub tui_log_path: Option<String>,
    /// Panic log path
    #[serde(default)]
    pub panic_log_path: Option<String>,
    /// Supervisor info
    pub supervisor: AgentInfo,
    /// Worker info
    pub workers: Vec<AgentInfo>,
    /// Epic ID if active
    pub epic_id: Option<String>,
    /// Explicit supervisor-pinned epic ID for display focus
    #[serde(default)]
    pub pinned_epic_id: Option<String>,
    /// Branch delivery route selected for this factory session.
    #[serde(default)]
    pub delivery_mode: cas_types::DeliveryMode,
    /// Workers deliberately parked by the supervisor for this factory session.
    ///
    /// The director reconciles this durable set into its in-memory hold gate.
    /// Session metadata is removed on clean shutdown, so holds survive a daemon
    /// restart of the same session but never cross into a different session.
    #[serde(default)]
    pub held_workers: Vec<String>,
    /// Most recent CAS MCP call made by this factory session's supervisor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_supervisor_mcp_call_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Persisted detector cadence and actionable-idle metric for this session.
    #[serde(default)]
    pub supervisor_stall: crate::ui::factory::director::SupervisorStallTracker,
    /// Project directory this session belongs to (for multi-project isolation)
    #[serde(default)]
    pub project_dir: Option<String>,
    /// Native Agent Teams team name (when Teams messaging is enabled)
    #[serde(default)]
    pub team_name: Option<String>,
}

impl SessionMetadata {
    /// Current per-session actionable-idle metric, including an active span.
    pub fn actionable_idle_minutes_at(&self, now: chrono::DateTime<chrono::Utc>) -> u64 {
        self.supervisor_stall.actionable_idle_minutes_at(now)
    }
}

/// Basic agent info for session metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Agent name
    pub name: String,
    /// Process ID
    pub pid: Option<u32>,
    /// Worktree path (if using worktrees)
    pub worktree_path: Option<String>,
}

/// Frame header for length-prefixed messages
pub const FRAME_HEADER_SIZE: usize = 4;

/// Maximum message size (16 MB)
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Encode a message with length prefix
pub fn encode_message<T: Serialize>(msg: &T) -> Result<Vec<u8>, serde_json::Error> {
    let json = serde_json::to_vec(msg)?;
    let len = json.len() as u32;
    let mut buf = Vec::with_capacity(FRAME_HEADER_SIZE + json.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&json);
    Ok(buf)
}

/// Decode message length from header
pub fn decode_length(header: &[u8; FRAME_HEADER_SIZE]) -> usize {
    u32::from_be_bytes(*header) as usize
}

#[cfg(test)]
mod tests {
    use crate::ui::factory::protocol::*;

    fn attributed_remote_operator() -> MessageAttribution {
        MessageAttribution {
            device_id: Some("device-123".to_string()),
            credential_id: Some("credential-456".to_string()),
            device_label: Some("Pippenz phone".to_string()),
            operator_label: Some("Pippenz".to_string()),
            controller_origin: Some("https://commander.example".to_string()),
            request_id: Some("request-789".to_string()),
        }
    }

    #[test]
    fn test_encode_decode_client_message() {
        let msg = ClientMessage::Input {
            pane_id: "worker-1".to_string(),
            data: vec![0x1b, 0x5b, 0x41], // Up arrow
        };

        let encoded = encode_message(&msg).unwrap();
        assert!(encoded.len() > FRAME_HEADER_SIZE);

        let len = decode_length(encoded[..FRAME_HEADER_SIZE].try_into().unwrap());
        assert_eq!(len, encoded.len() - FRAME_HEADER_SIZE);

        let decoded: ClientMessage = serde_json::from_slice(&encoded[FRAME_HEADER_SIZE..]).unwrap();

        match decoded {
            ClientMessage::Input { pane_id, data } => {
                assert_eq!(pane_id, "worker-1");
                assert_eq!(data, vec![0x1b, 0x5b, 0x41]);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_encode_decode_daemon_message() {
        let msg = DaemonMessage::Output {
            pane_id: "supervisor".to_string(),
            data: b"Hello, world!\n".to_vec(),
        };

        let encoded = encode_message(&msg).unwrap();
        let len = decode_length(encoded[..FRAME_HEADER_SIZE].try_into().unwrap());

        let decoded: DaemonMessage =
            serde_json::from_slice(&encoded[FRAME_HEADER_SIZE..FRAME_HEADER_SIZE + len]).unwrap();

        match decoded {
            DaemonMessage::Output { pane_id, data } => {
                assert_eq!(pane_id, "supervisor");
                assert_eq!(data, b"Hello, world!\n");
            }
            _ => panic!("Wrong message type"),
        }
    }

    /// T2 (cas-4cae): SpawnWorkers must carry per-worker specs.
    /// This test fails until protocol.rs, PendingSpawn, and finish_worker_spawn are updated.
    #[test]
    fn spawn_workers_with_spec_round_trips_through_wire() {
        use cas_mux::{SupervisorCli, WorkerSpec};
        let spec = WorkerSpec {
            name: Some("alice".to_string()),
            cli: SupervisorCli::Codex,
            model: Some("gpt-5.5".to_string()),
            effort: Some(cas_mux::Effort::Medium),
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        };
        let msg = ClientMessage::SpawnWorkers {
            count: 1,
            names: vec!["alice".to_string()],
            specs: vec![Some(spec)],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::SpawnWorkers {
                count,
                names,
                specs,
            } => {
                assert_eq!(count, 1);
                assert_eq!(names, vec!["alice"]);
                assert_eq!(specs.len(), 1);
                let s = specs[0].as_ref().unwrap();
                assert_eq!(
                    s.name.as_deref(),
                    Some("alice"),
                    "WorkerSpec.name must survive wire round-trip"
                );
                assert_eq!(s.cli, SupervisorCli::Codex);
                assert_eq!(s.model.as_deref(), Some("gpt-5.5"));
                assert_eq!(s.effort, Some(cas_mux::Effort::Medium));
            }
            _ => panic!("Wrong message type decoded"),
        }
    }

    /// Backwards compat: old clients sending SpawnWorkers without specs must decode cleanly.
    #[test]
    fn spawn_workers_without_specs_field_is_backwards_compatible() {
        // Simulate a legacy wire message with no "specs" field
        let json = r#"{"SpawnWorkers":{"count":2,"names":["bob","carol"]}}"#;
        let decoded: ClientMessage = serde_json::from_str(json).unwrap();
        match decoded {
            ClientMessage::SpawnWorkers {
                count,
                names,
                specs,
            } => {
                assert_eq!(count, 2);
                assert_eq!(names, vec!["bob", "carol"]);
                assert!(
                    specs.is_empty(),
                    "missing specs field should default to empty vec"
                );
            }
            _ => panic!("Wrong message type decoded"),
        }
    }

    #[test]
    fn legacy_unit_interrupt_wire_shape_is_unchanged() {
        let json = serde_json::to_string(&ClientMessage::Interrupt).unwrap();
        assert_eq!(json, r#""Interrupt""#);
        assert!(matches!(
            serde_json::from_str::<ClientMessage>(&json).unwrap(),
            ClientMessage::Interrupt
        ));
    }

    #[test]
    fn targeted_interrupt_is_a_separately_named_additive_variant() {
        let json = serde_json::to_string(&ClientMessage::InterruptPane {
            pane_id: "worker-1".to_string(),
        })
        .unwrap();
        assert_eq!(json, r#"{"InterruptPane":{"pane_id":"worker-1"}}"#);
        assert!(matches!(
            serde_json::from_str::<ClientMessage>(&json).unwrap(),
            ClientMessage::InterruptPane { pane_id } if pane_id == "worker-1"
        ));
    }

    #[test]
    fn semantic_message_wire_contract_requires_explicit_attribution() {
        let msg = ClientMessage::SendMessage {
            target: "worker-1".to_string(),
            text: "Please checkpoint now".to_string(),
            summary: Some("checkpoint request".to_string()),
            urgent: false,
            attribution: attributed_remote_operator(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::SendMessage {
                target,
                text,
                summary,
                urgent,
                attribution,
            } => {
                assert_eq!(target, "worker-1");
                assert_eq!(text, "Please checkpoint now");
                assert_eq!(summary.as_deref(), Some("checkpoint request"));
                assert!(!urgent);
                assert_eq!(attribution.device_id.as_deref(), Some("device-123"));
                assert_eq!(attribution.operator_label.as_deref(), Some("Pippenz"));
            }
            _ => panic!("Wrong message type decoded"),
        }

        let missing_attribution =
            r#"{"SendMessage":{"target":"worker-1","text":"hello","summary":null,"urgent":false}}"#;
        assert!(
            serde_json::from_str::<ClientMessage>(missing_attribution).is_err(),
            "attribution is a required part of the wire contract"
        );
    }

    #[test]
    fn unavailable_attribution_is_explicit_and_never_supervisor() {
        let attribution = MessageAttribution {
            device_id: None,
            credential_id: None,
            device_label: None,
            operator_label: None,
            controller_origin: None,
            request_id: None,
        };
        let json = serde_json::to_value(&attribution).unwrap();
        for field in [
            "device_id",
            "credential_id",
            "device_label",
            "operator_label",
            "controller_origin",
            "request_id",
        ] {
            assert_eq!(json.get(field), Some(&serde_json::Value::Null));
        }
        assert_ne!(attribution.queue_source(), "supervisor");
        assert_ne!(attribution.queue_source(), "mcp");
    }

    #[test]
    fn welcome_negotiates_version_and_capabilities_additively() {
        let legacy_json = r#"{"Welcome":{"session_name":"factory-1","state":{"focused_pane":null,"panes":[],"epic_id":null,"epic_title":null,"cols":120,"rows":40},"scrollback":null}}"#;
        let decoded: DaemonMessage = serde_json::from_str(legacy_json).unwrap();
        match decoded {
            DaemonMessage::Welcome {
                protocol_version,
                capabilities,
                ..
            } => {
                assert_eq!(protocol_version, LEGACY_PROTOCOL_VERSION);
                assert!(capabilities.is_empty());
            }
            _ => panic!("Wrong message type decoded"),
        }

        let current = DaemonMessage::Welcome {
            session_name: "factory-1".to_string(),
            state: SessionState {
                focused_pane: None,
                panes: Vec::new(),
                epic_id: None,
                epic_title: None,
                cols: 120,
                rows: 40,
            },
            scrollback: None,
            protocol_version: PROTOCOL_VERSION,
            capabilities: daemon_capabilities(),
            pane_bootstrap: Vec::new(),
        };
        let json = serde_json::to_value(&current).unwrap();
        assert_eq!(
            json.pointer("/Welcome/protocol_version"),
            Some(&serde_json::json!(PROTOCOL_VERSION))
        );
        assert!(daemon_capabilities().contains(&ProtocolCapability::TargetedInterrupt));
        assert!(daemon_capabilities().contains(&ProtocolCapability::AttributedSendMessage));
        assert!(daemon_capabilities().contains(&ProtocolCapability::AuthoritativePaneKeyframes));
        assert!(daemon_capabilities().contains(&ProtocolCapability::PagedScrollback));
    }

    #[test]
    fn keyframe_and_scrollback_requests_are_additive_wire_messages() {
        let keyframe = ClientMessage::RequestPaneKeyframe {
            pane_id: "supervisor".into(),
        };
        let request = ClientMessage::ScrollbackRequest {
            pane_id: "worker-1".into(),
            generation: 42,
            start_row: 100,
            count: 200,
        };

        assert_eq!(
            serde_json::to_value(keyframe).unwrap(),
            serde_json::json!({"RequestPaneKeyframe":{"pane_id":"supervisor"}})
        );
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"ScrollbackRequest":{"pane_id":"worker-1","generation":42,"start_row":100,"count":200}})
        );
    }

    /// New-daemon / old-client direction: serde's default unknown-field
    /// tolerance lets a legacy TUI decode the v2 Welcome without knowing the
    /// additive negotiation fields.
    #[test]
    fn legacy_client_decodes_current_welcome_by_ignoring_additive_fields() {
        #[derive(Debug, Deserialize)]
        enum LegacyDaemonMessage {
            Welcome {
                session_name: String,
                state: SessionState,
                scrollback: Option<HashMap<String, Vec<Vec<u8>>>>,
            },
        }

        let current = DaemonMessage::Welcome {
            session_name: "factory-1".to_string(),
            state: SessionState {
                focused_pane: None,
                panes: Vec::new(),
                epic_id: None,
                epic_title: None,
                cols: 120,
                rows: 40,
            },
            scrollback: None,
            protocol_version: PROTOCOL_VERSION,
            capabilities: daemon_capabilities(),
            pane_bootstrap: Vec::new(),
        };
        let json = serde_json::to_string(&current).unwrap();
        let legacy: LegacyDaemonMessage = serde_json::from_str(&json).unwrap();
        match legacy {
            LegacyDaemonMessage::Welcome {
                session_name,
                state,
                scrollback,
            } => {
                assert_eq!(session_name, "factory-1");
                assert_eq!(state.cols, 120);
                assert!(scrollback.is_none());
            }
        }
    }

    /// New-client / old-daemon direction: an old decoder rejects only the
    /// unknown additive control frame. The next legacy frame still decodes,
    /// matching both daemon transport loops' per-message error handling.
    #[test]
    fn legacy_daemon_rejects_new_control_without_poisoning_following_messages() {
        #[derive(Debug, Deserialize)]
        enum LegacyClientMessage {
            Focus { pane_id: String },
            Interrupt,
        }

        for new_control in [
            serde_json::to_string(&ClientMessage::InterruptPane {
                pane_id: "worker-1".to_string(),
            })
            .unwrap(),
            serde_json::to_string(&ClientMessage::SendMessage {
                target: "worker-1".to_string(),
                text: "hello".to_string(),
                summary: None,
                urgent: false,
                attribution: attributed_remote_operator(),
            })
            .unwrap(),
        ] {
            assert!(serde_json::from_str::<LegacyClientMessage>(&new_control).is_err());
            let next: LegacyClientMessage =
                serde_json::from_str(r#"{"Focus":{"pane_id":"worker-1"}}"#).unwrap();
            assert!(matches!(
                next,
                LegacyClientMessage::Focus { pane_id } if pane_id == "worker-1"
            ));
        }

        assert!(matches!(
            serde_json::from_str::<LegacyClientMessage>(r#""Interrupt""#).unwrap(),
            LegacyClientMessage::Interrupt
        ));
    }

    #[test]
    fn unknown_client_message_remains_a_non_destructive_decode_error() {
        let unknown = r#"{"FutureControl":{"value":1}}"#;
        assert!(serde_json::from_str::<ClientMessage>(unknown).is_err());

        let known_after_unknown = r#"{"Focus":{"pane_id":"worker-1"}}"#;
        assert!(matches!(
            serde_json::from_str::<ClientMessage>(known_after_unknown).unwrap(),
            ClientMessage::Focus { pane_id } if pane_id == "worker-1"
        ));
    }

    #[test]
    fn test_pane_kind_conversion() {
        assert_eq!(PaneKind::from(cas_mux::PaneKind::Worker), PaneKind::Worker);
        assert_eq!(
            PaneKind::from(cas_mux::PaneKind::Supervisor),
            PaneKind::Supervisor
        );
        assert_eq!(
            PaneKind::from(cas_mux::PaneKind::Director),
            PaneKind::Director
        );
    }
}
