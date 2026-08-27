//! CAS Terminal Multiplexer
//!
//! A terminal multiplexer built on ghostty_vt + ratatui for CAS factory mode.
//! Provides direct PTY control for reliable prompt injection into Claude instances.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                      Multiplexer                         │
//! │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌───────────────┐ │
//! │  │  Pane   │ │  Pane   │ │  Pane   │ │     Pane      │ │
//! │  │ worker1 │ │ worker2 │ │  super  │ │   director    │ │
//! │  │  (PTY)  │ │  (PTY)  │ │  (PTY)  │ │   (native)    │ │
//! │  └─────────┘ └─────────┘ └─────────┘ └───────────────┘ │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! Each pane with a PTY has:
//! - A ghostty_vt Terminal for parsing and state management
//! - A direct write handle for prompt injection
//! - An associated agent name for targeting
//!
//! # Components
//!
//! - **ghostty_vt**: Handles terminal emulation (escape sequences, cursor, colors)
//! - **portable-pty**: Manages PTY processes
//! - **ratatui**: Renders the TUI output

mod backend;
mod error;
mod harness;
mod input_stream;
mod mux;
mod opencode;
mod pane;
mod pty;
mod render;
mod spec;

pub use backend::{Backend, SupervisorLaunchConfig, WorkerLaunchConfig};
pub use error::{Error, Result};
pub use harness::{HarnessCapabilities, SupervisorCli};
pub use input_stream::{BracketedPasteTracker, StreamByteClass, key_stream_marks_submit};
pub use mux::{InjectOutcome, Mux, MuxConfig, MuxEvent};
pub use opencode::{
    OPENCODE_PLUGIN_FILE_NAME, OPENCODE_PLUGIN_SOURCE, OPENCODE_STATE_DIRECTORY,
    OPENCODE_STATE_SCHEMA_VERSION, OpenCodeEventOutcome, OpenCodeLiveness, OpenCodeLivenessVerdict,
    OpenCodeProjectionSpec, OpenCodeRole, OpenCodeSessionEvent, OpenCodeSessionEventKind,
    OpenCodeSessionState, OpenCodeStatus, OpenCodeToolAttribution, load_opencode_session_state,
    merge_opencode_projection, opencode_session_state_path, persist_opencode_plugin,
    persist_opencode_session_state, render_opencode_config, valid_opencode_session_id,
};
pub use pane::TerminalSnapshot;
pub use pane::{Pane, PaneBackend, PaneId, PaneKind, UserInputKind};
pub use pty::{
    ContractRole, claude_supervisor_contract, claude_worker_contract, missing_contract_elements,
    rendered_contract_surface,
};
pub use pty::{Pty, PtyConfig, PtyEvent, TeamsSpawnConfig};
pub use render::{LayoutDirection, Renderer};
pub use spec::{Effort, WorkerSpec};
