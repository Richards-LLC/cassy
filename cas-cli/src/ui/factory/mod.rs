//! Factory TUI - Native terminal multiplexer for Cassy factory mode
//!
//! Spawns and manages worker/supervisor agents directly using cas-mux,
//! with an integrated Director panel for monitoring Cassy tasks/agents/activity.
//!
//! # Architecture
//!
//! The factory TUI uses a daemon + client architecture for session persistence:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    FACTORY DAEMON                        │
//! │  (persistent process, owns PTYs, manages sessions)      │
//! │                                                         │
//! │  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐    │
//! │  │ PTY: super   │ │ PTY: worker1 │ │ PTY: worker2 │    │
//! │  │ ┌──────────┐ │ │ ┌──────────┐ │ │ ┌──────────┐ │    │
//! │  │ │ Agent    │ │ │ │ Agent    │ │ │ │ Agent    │ │    │
//! │  │ └──────────┘ │ │ └──────────┘ │ │ └──────────┘ │    │
//! │  └──────────────┘ └──────────────┘ └──────────────┘    │
//! │                                                         │
//! │  Socket: ~/.cas/factory-{session}.sock                  │
//! └─────────────────────────────────────────────────────────┘
//!            ▲
//!            │ attach/detach (socket protocol)
//!            ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │                    TUI (client)                          │
//! │  - Renders terminal output from daemon                  │
//! │  - Sends keyboard input to daemon                       │
//! │  - Can disconnect without killing daemon                │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! # Layout
//!
//! ```text
//! ┌──────────────────┬──────────────────┬────────────────────────────┐
//! │    WORKERS       │   SUPERVISOR     │      DIRECTOR              │
//! │   (Stacked)      │                  │    (Native Widgets)        │
//! │ ┌──────────────┐ │ ┌──────────────┐ │  ┌────────────────────┐   │
//! │ │ swift-fox    │ │ │ wise-eagle   │ │  │ Tasks (ready/prog) │   │
//! │ │ (Agent CLI)  │ │ │ (Agent CLI)  │ │  ├────────────────────┤   │
//! │ ├──────────────┤ │ │              │ │  │ Agents (status)    │   │
//! │ │ calm-owl     │ │ │              │ │  ├────────────────────┤   │
//! │ │ (Agent CLI)  │ │ │              │ │  │ Activity (events)  │   │
//! │ └──────────────┘ │ └──────────────┘ │  └────────────────────┘   │
//! └──────────────────┴──────────────────┴────────────────────────────┘
//! ```
//!
//! # Key constraints
//!
//! - Workers and supervisor accept keyboard input when focused
//! - Inject mode ('i') allows programmatic prompt injection to any pane
//! - Detach with Ctrl+D keeps daemon running

mod app;
mod boot;
mod buffer_backend;
mod client;
pub(crate) mod daemon;
mod director;
pub(crate) use director::effective_stall_threshold_secs;
pub(crate) mod cgroup;
mod input;
mod layout;
mod notification;
pub(crate) mod orphan_gc;
pub(crate) mod phoenix;
pub(crate) mod process_groups;
mod protocol;
pub mod renderer;
/// cas-7c93 (GH #87): sanctioned lifecycle for servers that outlive a task.
pub(crate) mod server_registry;
mod session;
mod status_bar;
pub(crate) use app::{
    persist_session_metadata_delivery_mode_at, persist_session_metadata_pinned_epic_id_at,
    persist_session_metadata_worker_hold_at, worker_holds_from_session_metadata_named,
};

/// The pinned/default epic is shared session state, not only a TUI concern.
/// Keep the private focus-source detail inside `app`; hook consumers only need
/// the resolved identifier for bounded domain conditioning.
pub(crate) fn preferred_epic_id_from_session_metadata() -> Option<String> {
    app::preferred_epic_focus_from_session_metadata().epic_id
}

/// The same resolution for a NAMED session rather than this process's own
/// (cas-5087).
///
/// Session metadata lives in one shared `~/.cas/sessions/` directory, so a
/// supervisor can read what another live supervisor on this clone declared it
/// is running. That is the whole point: `worker_status` is read before a gate,
/// and "who else is here" is only half the answer without "and what are they
/// in the middle of".
pub(crate) fn preferred_epic_id_from_session_metadata_named(session: &str) -> Option<String> {
    app::preferred_epic_focus_from_session_metadata_named(session).epic_id
}
// cas-bd9d: the parity conformance gate drives these launch intro-prompt paths.
pub use app::{FactoryApp, FactoryConfig};
#[cfg(test)]
pub(crate) use app::{queue_codex_worker_intro_prompt, queue_supervisor_intro_prompt};
pub use boot::{BootConfig, run_boot_screen_client};
pub use client::{
    attach, find_session_for_project, list_session_summaries, list_session_summaries_for_project,
    list_sessions, list_sessions_for_project,
};
/// The delivery wake gate's argument and verdict types (cas-5087), exported
/// alongside [`FactoryDaemon::supervisor_wake_decision`] so acceptance tests
/// and diagnostics can drive the real gate instead of restating its rules.
pub use daemon::runtime::queue_and_events::{
    PaneWakeState, SILENCE_FOR_ACTIVE_RECIPIENT_WAKE, SupervisorWakeClass, ToolCallEvidence,
    WakeDecision, WakeOutcome, WakeSender,
};
pub use daemon::{
    DaemonConfig, DaemonInitPhase, FactoryDaemon, ForkFirstResult, ForkResult, daemonize,
    fork_first_daemon, fork_into_daemon, run_daemon, run_daemon_after_fork,
    run_daemon_with_boot_progress,
};
pub use layout::{Direction, MissionControlLayout, PANE_SIDECAR, PaneGrid};
pub use notification::{Notifier, NotifyBackend, NotifyConfig};
pub(crate) use protocol::COMMANDER_REPLAY_BYTES_PER_PANE;
pub use protocol::{
    ClientMessage, DaemonMessage, MessageAttribution, PROTOCOL_VERSION, PaneBootstrap, PaneInfo,
    PaneKind, PaneSizeAuthority, ProtocolCapability, SessionMetadata, SessionState,
    daemon_capabilities,
};
pub use renderer::{FactoryViewMode, MissionControlFocus};
pub use session::{
    SessionInfo, SessionManager, create_metadata, daemon_log_path, daemon_trace_log_path,
    generate_session_name, metadata_path, panic_log_path, session_log_dir, socket_path,
    tui_log_path,
};
use std::io;
use std::path::Path;

use crossterm::{execute, terminal::SetTitle};

/// Build the terminal title string for factory mode
///
/// Format: "Cassy Factory - [Project] - [Epic]" or "Cassy Factory - [Project]" if no epic
fn build_terminal_title(project_dir: &Path, epic_title: Option<&str>) -> String {
    let project_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown");

    match epic_title {
        Some(epic) => format!("Cassy Factory - {project_name} - {epic}"),
        None => format!("Cassy Factory - {project_name}"),
    }
}

/// Set the terminal window/tab title
///
/// Uses OSC escape sequence to set the title, supported by most terminal emulators.
pub fn set_terminal_title(project_dir: &Path, epic_title: Option<&str>) {
    let title = build_terminal_title(project_dir, epic_title);
    let _ = execute!(io::stdout(), SetTitle(title));
}
