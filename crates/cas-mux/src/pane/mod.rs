//! Pane abstraction using ghostty_vt for terminal emulation
//!
//! A pane combines:
//! - A PTY process (optional - director pane is native)
//! - A ghostty_vt Terminal for state management
//! - Metadata (agent name, role, etc.)

mod snapshot;
mod style;
mod tests;

use crate::error::{Error, Result};
use crate::harness::SupervisorCli;
use crate::pane::style::{cell_style_to_ratatui, debug_log_enabled};
use crate::pty::{Pty, PtyConfig, PtyEvent, TeamsSpawnConfig};
pub use cas_factory_protocol::TerminalSnapshot;
use ghostty_vt::{CellStyle, Rgb, Terminal};
use ratatui::text::{Line, Span};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use cas_recording::{RecordingWriter, WriterConfig};

/// How user input is classified for turn-submit side effects (cas-7f6f).
///
/// One explicit submit API across terminal, GUI, WebSocket, and relay surfaces.
/// Structured paste/drop never marks turn-in-flight even when CR/LF is embedded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInputKind {
    /// Keyboard / keystroke stream. Lone CR/LF (or a multi-byte keystream
    /// chunk ending in CR/LF) marks a true prompt submit.
    KeyStream,
    /// Bracketed paste, image drop, or other structured non-keyboard input.
    /// Never marks turn-in-flight.
    StructuredPaste,
}

/// Unique identifier for a pane
pub type PaneId = String;

/// The kind of pane
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneKind {
    /// Worker agent (Claude/Codex CLI)
    Worker,
    /// Supervisor agent (Claude/Codex CLI)
    Supervisor,
    /// Director (native TUI, no PTY)
    Director,
    /// Generic shell
    Shell,
}

impl PaneKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Supervisor => "supervisor",
            Self::Director => "director",
            Self::Shell => "shell",
        }
    }
}

/// Backend for a pane — either a PTY (Claude/Codex interactive) or
/// none (director pane).
pub enum PaneBackend {
    /// No backend (director pane — rendered natively)
    None,
    /// PTY-based interactive terminal (Claude, Codex)
    Pty(Pty),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ComposerEscapeState {
    #[default]
    Normal,
    Escape,
    ControlSequence,
}

/// A pane in the multiplexer
pub struct Pane {
    /// Unique identifier (usually agent name)
    id: PaneId,
    /// What kind of pane
    kind: PaneKind,
    /// The ghostty_vt terminal (handles escape sequences, cursor, colors)
    pub(crate) terminal: Terminal,
    /// Process backend
    backend: PaneBackend,
    /// Whether this pane has focus
    focused: bool,
    /// Title for display
    title: String,
    /// Color for the pane border (hex)
    color: Option<String>,
    /// Whether the process has exited
    exited: bool,
    /// Exit code if exited
    exit_code: Option<i32>,
    /// Terminal dimensions
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    /// Optional recording writer for session capture
    recorder: Option<Arc<Mutex<RecordingWriter>>>,
    /// Whether to force all rows dirty on next take (for new client sync)
    force_all_dirty: bool,
    /// Last known total scrollback lines (for scroll detection)
    pub(crate) last_total_scrollback: u32,
    /// Sequence counter for incremental updates (pane-scoped)
    pub(crate) seq_counter: u64,
    /// Whether the user has scrolled up from the bottom
    user_scrolled: bool,
    /// Number of new output lines received while user was scrolled up
    new_lines_below: u32,
    /// Reusable scratch buffer for drain_output (avoids 65KB alloc per poll)
    drain_buf: Vec<u8>,
    /// Total bytes of output received from the process (for readiness detection)
    total_bytes_received: u64,
    /// When this pane was created (for startup grace period)
    created_at: std::time::Instant,
    /// Grok-authoritative in-flight turn flag (cas-7f6f).
    ///
    /// Set at control points that start a turn (true prompt submit / inject via
    /// [`UserInputKind::KeyStream`]); cleared on cancel (`break_turn` /
    /// `interrupt`) or authoritative harness completion
    /// ([`Self::mark_turn_completed`] from Grok `events.jsonl` `turn_ended`).
    ///
    /// Claude and Codex expose no normal-completion event to the mux. For those
    /// harnesses this remains set after a normally completed turn, so it only
    /// means "a submit occurred since the last explicit clear" and MUST NOT be
    /// used as a busy/idle signal. Production reads are scoped to Grok.
    /// Not set by output redraws, paste/drop, or generic SGR clicks.
    /// Not cleared by PTY quiet timers (long tool waits stay in-flight).
    turn_in_flight: std::sync::atomic::AtomicBool,
    /// Harness session id (`CAS_SESSION_ID` / Grok `--session-id`).
    /// Used to locate `~/.grok/sessions/*/<id>/events.jsonl` for turn completion.
    harness_session_id: std::sync::Mutex<Option<String>>,
    /// Byte offset into the harness events file when the current turn started.
    /// Only events after this offset can complete the turn.
    turn_events_byte_offset: std::sync::Mutex<Option<u64>>,
    /// Test/override path for harness events (skips session-id lookup).
    harness_events_path_override: std::sync::Mutex<Option<PathBuf>>,
    /// Whether the inner process is currently in alt-screen mode.
    ///
    /// Tracked by watching for DEC private mode set/reset sequences in `feed()`:
    /// - Enter: ESC [ ? 1049 h  /  ESC [ ? 1047 h  /  ESC [ ? 47 h
    /// - Exit:  ESC [ ? 1049 l  /  ESC [ ? 1047 l  /  ESC [ ? 47 l
    ///
    /// Used by the factory UI to decide whether wheel events should be
    /// forwarded to the inner process (alt-screen, no scrollback) or handled
    /// by `Pane::scroll` (normal screen, scrollback available).
    in_alt_screen: bool,
    /// Whether this pane has ever received an OSC 8 hyperlink sequence.
    has_hyperlinks: bool,
    /// Partial DEC private mode sequence carried over from the previous `feed()` chunk.
    ///
    /// PTY output arrives in arbitrary-sized chunks; a DEC escape sequence such as
    /// `ESC [ ? 1049 h` can be split across two consecutive reads.  Keeping up to
    /// `PARTIAL_ESC_CAP` (16) bytes of a trailing partial sequence and prepending
    /// them to the next chunk means `update_alt_screen` always sees whole sequences.
    partial_esc: Vec<u8>,
    /// Partial OSC 8 introducer carried over from the previous `feed()` chunk.
    partial_osc8: Vec<u8>,
    /// Interactive harness for this pane (Claude / Codex / Grok).
    ///
    /// Drives harness-aware turn cancel via [`Pane::break_turn`] (cas-7f6f).
    /// Shell and director panes default to [`SupervisorCli::Claude`] (Esc is a
    /// harmless no-op on bare shells).
    harness: SupervisorCli,
    /// Streaming bracketed-paste tracker for KeyStream submit detection (cas-4b99).
    ///
    /// Survives split `\x1b[200~` / `\x1b[201~` packets so embedded CR/LF inside
    /// a paste never marks turn-in-flight. StructuredPaste does not advance this.
    paste_tracker: std::sync::Mutex<crate::input_stream::BracketedPasteTracker>,
    /// Whether an attached operator has unsubmitted input in this pane's
    /// composer (cas-1a4d).
    ///
    /// Sticky after printable input/paste until a true submit or an explicit
    /// clear/cancel. Panes that never receive attached-client input remain
    /// clean, preserving immediate worker-to-worker injection.
    composer_dirty: std::sync::atomic::AtomicBool,
    /// When the current dirty-composer interval began. This lets the mux
    /// enforce bounded deferral without retaining the payload itself.
    composer_dirty_since: std::sync::Mutex<Option<std::time::Instant>>,
}

impl Pane {
    /// Create a new pane with a specific backend.
    fn new_with_backend(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: PaneKind,
        backend: PaneBackend,
        rows: u16,
        cols: u16,
        harness: SupervisorCli,
    ) -> Result<Self> {
        let id = id.into();
        let mut terminal = Terminal::new(rows, cols).map_err(|e| Error::terminal(e.to_string()))?;
        terminal.set_default_colors(Rgb { r: 0, g: 0, b: 0 }, Rgb { r: 0, g: 0, b: 0 });
        let info = terminal.scrollback_info();
        Ok(Self {
            title: title.into(),
            id,
            kind,
            terminal,
            backend,
            focused: false,
            color: None,
            exited: false,
            exit_code: None,
            rows,
            cols,
            recorder: None,
            force_all_dirty: true,
            last_total_scrollback: info.total_scrollback,
            seq_counter: 0,
            user_scrolled: false,
            new_lines_below: 0,
            drain_buf: Vec::with_capacity(65536),
            total_bytes_received: 0,
            created_at: std::time::Instant::now(),
            turn_in_flight: std::sync::atomic::AtomicBool::new(false),
            harness_session_id: std::sync::Mutex::new(None),
            turn_events_byte_offset: std::sync::Mutex::new(None),
            harness_events_path_override: std::sync::Mutex::new(None),
            in_alt_screen: false,
            has_hyperlinks: false,
            partial_esc: Vec::new(),
            partial_osc8: Vec::new(),
            harness,
            paste_tracker: std::sync::Mutex::new(crate::input_stream::BracketedPasteTracker::new()),
            composer_dirty: std::sync::atomic::AtomicBool::new(false),
            composer_dirty_since: std::sync::Mutex::new(None),
        })
    }

    /// Create a new pane with a PTY
    pub fn with_pty(
        id: impl Into<String>,
        kind: PaneKind,
        pty: Pty,
        rows: u16,
        cols: u16,
        harness: SupervisorCli,
    ) -> Result<Self> {
        let id_str: String = id.into();
        Self::new_with_backend(
            id_str.clone(),
            id_str,
            kind,
            PaneBackend::Pty(pty),
            rows,
            cols,
            harness,
        )
    }

    /// Create a director pane (no PTY)
    pub fn director(id: impl Into<String>, rows: u16, cols: u16) -> Result<Self> {
        let id_str: String = id.into();
        Self::new_with_backend(
            id_str,
            "Director",
            PaneKind::Director,
            PaneBackend::None,
            rows,
            cols,
            SupervisorCli::Claude,
        )
    }

    /// Create a shell pane running the user's default shell (or a specific command).
    pub fn shell(
        name: &str,
        cwd: PathBuf,
        shell_command: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        let shell = shell_command
            .map(|s| s.to_string())
            .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()));

        let config = PtyConfig {
            command: shell,
            args: vec![],
            cwd: Some(cwd),
            env: vec![],
            rows,
            cols,
        };
        let pty = Pty::spawn(name, config)?;
        // Shell has no agent harness; Claude cancel-key (Esc) is a no-op on shells.
        Self::with_pty(
            name,
            PaneKind::Shell,
            pty,
            rows,
            cols,
            SupervisorCli::Claude,
        )
    }

    /// Build the `PtyConfig` that `worker()` would spawn, without actually
    /// spawning a process. Used by `Mux::factory_pane_configs` and tests.
    #[allow(clippy::too_many_arguments)]
    pub fn build_worker_config(
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: &str,
        supervisor_cli: SupervisorCli,
        cli: SupervisorCli,
        model: Option<&str>,
        effort: Option<&str>,
        config_dir: Option<&str>,
        config_dir_source: Option<&str>,
        teams: Option<&TeamsSpawnConfig>,
    ) -> PtyConfig {
        let mut config = match cli {
            SupervisorCli::Claude => PtyConfig::claude(
                name,
                "worker",
                cwd,
                cas_root,
                Some(supervisor_name),
                None,
                model,
                effort,
                teams,
            ),
            SupervisorCli::Codex => PtyConfig::codex(
                name,
                "worker",
                cwd,
                cas_root,
                Some(supervisor_name),
                None,
                model,
                effort,
                teams,
            ),
            // cas-6569 (EPIC cas-8888, Phase 2): real driver — see
            // PtyConfig::grok's doc comment (crates/cas-pty/src/pty.rs) for
            // the verified flag set and the MCP/context-injection design.
            SupervisorCli::Grok => PtyConfig::grok(
                name,
                "worker",
                cwd,
                cas_root,
                Some(supervisor_name),
                None,
                model,
                effort,
                teams,
            ),
        };
        if cli == SupervisorCli::Claude {
            config.apply_claude_config_dir(config_dir, config_dir_source);
        }
        config.env.push((
            "CAS_FACTORY_SUPERVISOR_CLI".to_string(),
            supervisor_cli.as_str().to_string(),
        ));
        // cas-6569 (EPIC cas-8888, Phase 2) SILENT SITE — audited and
        // RESOLVED (was left open in Phase 1): confirmed against the real
        // grok 0.2.93 binary that it has NO `-c`/config-override flag at
        // all — `grok mcp add` only writes to persistent
        // ~/.grok/config.toml, there's no per-launch equivalent. This `-c
        // mcp_servers.cs.env.*` arg is Codex-specific syntax and stays
        // `== Codex` intentionally: Grok gets CAS_FACTORY_SUPERVISOR_CLI
        // the same way Claude already does — as a plain env var on the
        // process itself (pushed just above), relying on ordinary
        // child-process env inheritance when Grok spawns `cas serve` per
        // its resolved MCP config. See PtyConfig::grok's doc comment
        // (crates/cas-pty/src/pty.rs) for the full verification trail.
        if cli == SupervisorCli::Codex {
            config.args.push("-c".to_string());
            config.args.push(format!(
                "mcp_servers.cs.env.CAS_FACTORY_SUPERVISOR_CLI=\"{}\"",
                supervisor_cli.as_str()
            ));
        }
        config
    }

    #[allow(clippy::too_many_arguments)]
    pub fn worker(
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: &str,
        supervisor_cli: SupervisorCli,
        cli: SupervisorCli,
        model: Option<&str>,
        effort: Option<&str>,
        config_dir: Option<&str>,
        config_dir_source: Option<&str>,
        rows: u16,
        cols: u16,
        teams: Option<&TeamsSpawnConfig>,
        factory_session: Option<&str>,
    ) -> Result<Self> {
        let mut config = Self::build_worker_config(
            name,
            cwd,
            cas_root,
            supervisor_name,
            supervisor_cli,
            cli,
            model,
            effort,
            config_dir,
            config_dir_source,
            teams,
        );
        push_factory_session_env(&mut config, cli, factory_session);
        let session_id = cas_session_id_from_config(&config);
        let pty = Pty::spawn(name, config)?;
        let pane = Self::with_pty(name, PaneKind::Worker, pty, rows, cols, cli)?;
        if let Some(sid) = session_id {
            pane.set_harness_session_id(sid);
        }
        Ok(pane)
    }

    /// Build the `PtyConfig` that `supervisor()` would spawn, without actually
    /// spawning a process. Used by `Mux::factory_pane_configs` and tests.
    #[allow(clippy::too_many_arguments)]
    pub fn build_supervisor_config(
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        cli: SupervisorCli,
        worker_cli: SupervisorCli,
        worker_names: &[String],
        model: Option<&str>,
        effort: Option<&str>,
        teams: Option<&TeamsSpawnConfig>,
    ) -> PtyConfig {
        let worker_cli_str = worker_cli.as_str();
        let worker_names_csv = if worker_names.is_empty() {
            None
        } else {
            Some(worker_names.join(","))
        };
        let mut config = match cli {
            SupervisorCli::Claude => PtyConfig::claude(
                name,
                "supervisor",
                cwd,
                cas_root,
                None,
                Some(worker_cli_str),
                model,
                effort,
                teams,
            ),
            SupervisorCli::Codex => PtyConfig::codex(
                name,
                "supervisor",
                cwd,
                cas_root,
                None,
                Some(worker_cli_str),
                model,
                effort,
                teams,
            ),
            // cas-6569 (EPIC cas-8888, Phase 2): real driver — see
            // PtyConfig::grok's doc comment. `cas grok` (the standalone
            // supervisor launcher CLI command) is still Phase 3's job
            // (cas-964a); this only wires the underlying PtyConfig so a
            // grok supervisor pane, however it gets spawned, launches
            // correctly.
            SupervisorCli::Grok => PtyConfig::grok(
                name,
                "supervisor",
                cwd,
                cas_root,
                None,
                Some(worker_cli_str),
                model,
                effort,
                teams,
            ),
        };
        Self::push_supervisor_env(&mut config.env, cli, &worker_names_csv);
        config
    }

    #[allow(clippy::too_many_arguments)]
    pub fn supervisor(
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        rows: u16,
        cols: u16,
        cli: SupervisorCli,
        worker_cli: SupervisorCli,
        worker_names: &[String],
        model: Option<&str>,
        effort: Option<&str>,
        teams: Option<&TeamsSpawnConfig>,
        factory_session: Option<&str>,
    ) -> Result<Self> {
        let mut config = Self::build_supervisor_config(
            name,
            cwd,
            cas_root,
            cli,
            worker_cli,
            worker_names,
            model,
            effort,
            teams,
        );
        push_factory_session_env(&mut config, cli, factory_session);
        let session_id = cas_session_id_from_config(&config);
        let pty = Pty::spawn(name, config)?;
        let pane = Self::with_pty(name, PaneKind::Supervisor, pty, rows, cols, cli)?;
        if let Some(sid) = session_id {
            pane.set_harness_session_id(sid);
        }
        Ok(pane)
    }

    fn push_supervisor_env(
        env: &mut Vec<(String, String)>,
        cli: SupervisorCli,
        worker_names_csv: &Option<String>,
    ) {
        env.push((
            "CAS_FACTORY_SUPERVISOR_CLI".to_string(),
            cli.as_str().to_string(),
        ));
        if let Some(csv) = worker_names_csv {
            env.push(("CAS_FACTORY_WORKER_NAMES".to_string(), csv.clone()));
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn kind(&self) -> &PaneKind {
        &self.kind
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn color(&self) -> Option<&str> {
        self.color.as_deref()
    }

    pub fn set_color(&mut self, color: impl Into<String>) {
        self.color = Some(color.into());
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn mark_all_dirty(&mut self) {
        self.force_all_dirty = true;
    }

    pub(crate) fn take_force_all_dirty(&mut self) -> bool {
        std::mem::take(&mut self.force_all_dirty)
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        self.terminal.cursor_position()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        if debug_log_enabled() {
            tracing::debug!(
                "Pane {}: resize from {}x{} to {}x{}",
                self.id,
                self.rows,
                self.cols,
                rows,
                cols
            );
        }
        self.terminal.resize(rows, cols).map_err(|e| {
            tracing::warn!("Pane {}: terminal.resize failed: {}", self.id, e);
            Error::terminal(e.to_string())
        })?;
        self.rows = rows;
        self.cols = cols;
        match &self.backend {
            PaneBackend::Pty(pty) => pty.resize(rows, cols)?,
            PaneBackend::None => {}
        }
        Ok(())
    }

    /// Maximum bytes kept in the partial-escape carry buffer.
    const PARTIAL_ESC_CAP: usize = 16;

    /// Scan `data` for DEC private mode sequences that toggle the alternate
    /// screen buffer and return the updated `in_alt_screen` state.
    ///
    /// Handles:
    /// - ESC [ ? 1049 h/l  (save-cursor + enter/leave alt-screen — most common)
    /// - ESC [ ? 1047 h/l  (use alternate screen buffer)
    /// - ESC [ ? 47 h/l    (older xterm alternate screen)
    ///
    /// This is a fast forward scan; only the *last* matching sequence in the
    /// data wins, which is correct: if a pane enters and exits alt-screen in
    /// the same chunk, the final state is what matters.
    ///
    /// NOTE: `data` may already have a partial sequence prepended by `feed()`
    /// (via `partial_esc`), so this function is kept pure (no `&self`) and the
    /// caller manages the carry buffer.
    fn update_alt_screen(data: &[u8], current: bool) -> bool {
        let mut state = current;
        let mut i = 0;
        while i < data.len() {
            // Fast-path: skip directly to the next ESC byte using SIMD memchr.
            // Typical PTY output (Claude Code, factory worker streams) is dense
            // ASCII with ~1 ESC per 50-200 bytes — this turns the steady-state
            // cost of the scanner from O(n) byte-by-byte into O(n/16) SIMD.
            //
            // Semantics-preserving: every byte that is not 0x1b would otherwise
            // hit the `data[i] != 0x1b => i += 1; continue;` branch, so jumping
            // straight to the next ESC is observationally identical.
            match memchr::memchr(0x1b, &data[i..]) {
                Some(off) => i += off,
                None => break,
            }
            // i now points at an ESC byte. Re-run the existing structural checks.
            if i + 1 >= data.len() || data[i + 1] != b'[' {
                i += 1;
                continue;
            }
            if i + 2 >= data.len() || data[i + 2] != b'?' {
                i += 1;
                continue;
            }
            // Scan past digits for the (first) parameter value.
            let param_start = i + 3;
            let mut j = param_start;
            while j < data.len() && data[j].is_ascii_digit() {
                j += 1;
            }
            if j >= data.len() || j == param_start {
                // Either truncated (j >= data.len()) or no digits — skip.
                i += 1;
                continue;
            }
            // Snapshot the first parameter before we (optionally) walk past
            // ECMA-48 §5.4.2 sub-parameters / additional CSI parameters.
            let param_end = j;
            // ECMA-48 §5.4.2: parameters may carry sub-parameters separated
            // by `:`, and multiple parameters may be joined by `;`. xterm
            // emitters routinely produce e.g. `\x1b[?1049;1h`. We don't
            // interpret the trailing parameters, but we must walk past any
            // run of `[0-9;:]` so the final byte check sees `h`/`l` rather
            // than the separator. (cas-e0b9 fix.)
            if j < data.len() && (data[j] == b';' || data[j] == b':') {
                while j < data.len()
                    && (data[j].is_ascii_digit() || data[j] == b';' || data[j] == b':')
                {
                    j += 1;
                }
                if j >= data.len() {
                    // Sequence is truncated mid-parameter — leave it for the
                    // next chunk via the carry buffer. Skip to next ESC.
                    i += 1;
                    continue;
                }
            }
            let final_byte = data[j];
            // Only care about h (set) or l (reset).
            if final_byte != b'h' && final_byte != b'l' {
                i += 1;
                continue;
            }
            // Parse the first parameter (ASCII digits only, bounded length —
            // safe). Sub-parameters/additional parameters are ignored: per
            // xterm semantics, the leading mode value is what controls the
            // alt-screen toggle.
            let param: u32 = data[param_start..param_end].iter().fold(0u32, |acc, &b| {
                acc.wrapping_mul(10).wrapping_add((b - b'0') as u32)
            });
            match (param, final_byte) {
                (47 | 1047 | 1049, b'h') => state = true,
                (47 | 1047 | 1049, b'l') => state = false,
                _ => {}
            }
            i = j + 1;
        }
        state
    }

    /// Bench-only re-export of `update_alt_screen`.
    ///
    /// Bench harnesses live outside the lib crate and only see `pub` items, so
    /// this thin wrapper exposes the otherwise-private scanner without
    /// widening the public surface. Not intended for production callers.
    #[doc(hidden)]
    pub fn update_alt_screen_for_bench(data: &[u8], current: bool) -> bool {
        Self::update_alt_screen(data, current)
    }

    /// Return the trailing bytes of `data` that look like the start of a DEC
    /// private mode sequence (i.e., `ESC`, `ESC [`, `ESC [ ?`, or
    /// `ESC [ ? {digits…}`), capped at `PARTIAL_ESC_CAP` bytes.
    ///
    /// This is called after `update_alt_screen` so that if `data` ends mid-
    /// sequence the carry buffer is populated and the next `feed()` call sees
    /// the whole sequence.
    fn trailing_dec_partial(data: &[u8]) -> Vec<u8> {
        let cap = Self::PARTIAL_ESC_CAP;
        // Walk backwards from the end looking for a lone ESC that could start
        // an incomplete DEC sequence.  We only care about the last up-to-cap bytes.
        let search_start = data.len().saturating_sub(cap);
        let slice = &data[search_start..];

        // Find the last ESC (0x1b) in the slice.
        let esc_pos = match slice.iter().rposition(|&b| b == 0x1b) {
            Some(p) => p,
            None => return Vec::new(),
        };

        let tail = &slice[esc_pos..];

        // Check whether the tail matches the prefix of a DEC private mode sequence.
        // Pattern: ESC [ ? {digits…}   — any strict prefix is "partial".
        // `;` and `:` are also accepted in the parameter body to keep ECMA-48
        // sub-parameter sequences (e.g. `ESC [ ? 1049 ; 1 h`) intact when the
        // chunk boundary lands inside the parameters. (cas-e0b9 fix.)
        let is_partial = match tail {
            // Bare ESC at end
            [0x1b] => true,
            // ESC [
            [0x1b, b'['] => true,
            // ESC [ ?
            [0x1b, b'[', b'?'] => true,
            // ESC [ ? {digits / sub-param separators…} — no terminator yet
            [0x1b, b'[', b'?', rest @ ..]
                if !rest.is_empty()
                    && rest
                        .iter()
                        .all(|b| b.is_ascii_digit() || *b == b';' || *b == b':') =>
            {
                true
            }
            _ => false,
        };

        if is_partial {
            tail.to_vec()
        } else {
            Vec::new()
        }
    }

    /// Whether the inner process is currently in alt-screen (alternate buffer) mode.
    ///
    /// When `true`, the pane's ghostty_vt scrollback is empty — scrolling the
    /// viewport is a no-op. Wheel events should be forwarded to the inner process
    /// as arrow-key input so it can scroll its own transcript.
    pub fn is_in_alt_screen(&self) -> bool {
        self.in_alt_screen
    }

    pub fn has_hyperlinks(&self) -> bool {
        self.has_hyperlinks
    }

    fn contains_osc8_introducer(data: &[u8]) -> bool {
        data.windows(4).any(|window| window == b"\x1b]8;")
    }

    fn update_hyperlink_presence(&mut self, data: &[u8]) {
        if self.has_hyperlinks {
            return;
        }

        // Scan the carried tail together with the new chunk, and carry the
        // tail of the COMBINED buffer — carrying from `data` alone loses
        // introducer bytes when a link is split across 3+ tiny feeds.
        let mut scan_buf = std::mem::take(&mut self.partial_osc8);
        scan_buf.extend_from_slice(data);
        self.has_hyperlinks = Self::contains_osc8_introducer(&scan_buf);

        if !self.has_hyperlinks {
            let keep = scan_buf.len().min(3);
            scan_buf.drain(..scan_buf.len() - keep);
            self.partial_osc8 = scan_buf;
        }
    }

    pub fn feed(&mut self, data: &[u8]) -> Result<()> {
        self.update_hyperlink_presence(data);

        // Track alt-screen mode transitions before handing data to the terminal.
        // If the previous chunk ended with an incomplete DEC sequence, prepend
        // those carry bytes so split sequences are always seen whole.
        if self.partial_esc.is_empty() {
            self.in_alt_screen = Self::update_alt_screen(data, self.in_alt_screen);
            self.partial_esc = Self::trailing_dec_partial(data);
        } else {
            let mut scan_buf = std::mem::take(&mut self.partial_esc);
            scan_buf.extend_from_slice(data);
            self.in_alt_screen = Self::update_alt_screen(&scan_buf, self.in_alt_screen);
            self.partial_esc = Self::trailing_dec_partial(data);
        }

        if self.user_scrolled {
            // Save scroll position before feeding new data
            let before = self.terminal.scrollback_info();
            let old_total = before.total_scrollback;
            let old_offset = before.viewport_offset;

            self.terminal
                .feed(data)
                .map_err(|e| Error::terminal(e.to_string()))?;

            let after = self.terminal.scrollback_info();
            let new_lines = after.total_scrollback.saturating_sub(old_total);
            if new_lines > 0 {
                self.new_lines_below = self.new_lines_below.saturating_add(new_lines);
            }

            // Preserve viewport: the user should see the same content as before feed.
            // Target offset = old_offset + new_lines (same absolute position, measured
            // from the new bottom which is now further away by new_lines).
            // The terminal may or may not auto-scroll after feed — check the actual
            // offset and only adjust the delta needed.
            let target_offset = old_offset.saturating_add(new_lines);
            let current_offset = after.viewport_offset;
            if current_offset != target_offset {
                // Positive delta = scroll down (toward bottom), negative = scroll up
                let delta = current_offset as i32 - target_offset as i32;
                let _ = self.terminal.scroll(delta);
            }

            Ok(())
        } else {
            self.terminal
                .feed(data)
                .map_err(|e| Error::terminal(e.to_string()))
        }
    }

    /// Strip literal cursor-position report echoes such as `^[[1;1R`.
    ///
    /// Some agent CLIs emit this as plain text when probing terminal support,
    /// which creates visual noise in pane output.
    fn strip_literal_cursor_reports(data: &[u8]) -> Cow<'_, [u8]> {
        let mut out: Option<Vec<u8>> = None;
        let mut i = 0usize;
        let mut last_emit = 0usize;

        while i < data.len() {
            if let Some(len) = Self::literal_cursor_report_len(&data[i..]) {
                let out_buf = out.get_or_insert_with(|| Vec::with_capacity(data.len()));
                out_buf.extend_from_slice(&data[last_emit..i]);
                i += len;
                last_emit = i;
                continue;
            }
            i += 1;
        }

        if let Some(mut out_buf) = out {
            out_buf.extend_from_slice(&data[last_emit..]);
            Cow::Owned(out_buf)
        } else {
            Cow::Borrowed(data)
        }
    }

    fn literal_cursor_report_len(data: &[u8]) -> Option<usize> {
        // Matches: ^[[<row>;<col>R
        if data.len() < 7 || data[0] != b'^' || data[1] != b'[' || data[2] != b'[' {
            return None;
        }

        let mut idx = 3;
        let row_start = idx;
        while idx < data.len() && data[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == row_start || idx >= data.len() || data[idx] != b';' {
            return None;
        }

        idx += 1;
        let col_start = idx;
        while idx < data.len() && data[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == col_start || idx >= data.len() || data[idx] != b'R' {
            return None;
        }

        Some(idx + 1)
    }

    pub fn dump_viewport(&self) -> Result<String> {
        self.terminal
            .dump_viewport()
            .map_err(|e| Error::terminal(e.to_string()))
    }

    pub fn dump_row(&self, row: u16) -> Result<String> {
        self.terminal
            .dump_viewport_row(row)
            .map_err(|e| Error::terminal(e.to_string()))
    }

    pub fn row_styles(&self, row: u16) -> Result<Vec<CellStyle>> {
        self.terminal
            .row_cell_styles(row)
            .map_err(|e| Error::terminal(e.to_string()))
    }

    pub fn row_as_line(&self, row: u16) -> Result<Line<'static>> {
        let text = self.dump_row(row)?;
        // Use style runs (pre-grouped by the VT) instead of per-cell styles
        // to avoid a separate O(cols) traversal + per-cell comparison.
        let runs = self
            .terminal
            .row_style_runs(row)
            .map_err(|e| Error::terminal(e.to_string()))?;

        if runs.is_empty() {
            return Ok(Line::from(vec![Span::raw(text)]));
        }

        let chars: Vec<char> = text.chars().collect();
        let mut spans = Vec::with_capacity(runs.len());

        for run in &runs {
            let start = run.start_col as usize;
            let end = (run.end_col as usize).min(chars.len());
            if start >= chars.len() {
                break;
            }
            let span_text: String = chars[start..end].iter().collect();
            let style = cell_style_to_ratatui(&run.style);
            spans.push(Span::styled(span_text, style));
        }

        if spans.is_empty() && !text.is_empty() {
            spans.push(Span::raw(text));
        }

        Ok(Line::from(spans))
    }

    pub fn row_hyperlinks(&self, row: u16) -> Vec<Option<String>> {
        if !self.has_hyperlinks {
            return Vec::new();
        }

        (0..self.cols)
            .map(|col| self.terminal.hyperlink_at(col + 1, row + 1))
            .collect()
    }

    pub fn viewport_as_lines(&self) -> Result<Vec<Line<'static>>> {
        let mut lines = Vec::with_capacity(self.rows as usize);
        for row in 0..self.rows {
            lines.push(self.row_as_line(row)?);
        }
        Ok(lines)
    }

    /// Record that the inner process has exited.
    ///
    /// Called internally from `poll()` and `drain_output()` when the PTY emits
    /// `PtyEvent::Exited` (or `PtyEvent::Error`, treated as an abnormal exit).
    /// Exposed publicly so external tests can simulate process termination on
    /// a non-PTY backend (e.g., `Pane::director`).
    ///
    /// Resets `in_alt_screen` to `false`. A TUI that exits abnormally (kill,
    /// panic) never gets a chance to emit `\x1b[?1049l`, so without this
    /// reset the flag would leak across the process boundary — leaving the
    /// next process (or simply a redraw of the pane) with mis-routed wheel
    /// events. (cas-e0b9 fix.)
    pub fn mark_exited(&mut self, code: Option<i32>) {
        self.exited = true;
        self.exit_code = code;
        self.in_alt_screen = false;
        // Drop any partial-sequence carry too: it belonged to the now-dead
        // process and cannot be completed by the next one.
        self.partial_esc.clear();
    }

    pub fn poll(&mut self) -> Option<PtyEvent> {
        let event = match &mut self.backend {
            PaneBackend::Pty(pty) => pty.try_recv(),
            PaneBackend::None => None,
        }?;

        match &event {
            PtyEvent::Output(data) => {
                let feed_data = Self::strip_literal_cursor_reports(data);
                if let Err(e) = self.feed(feed_data.as_ref()) {
                    tracing::warn!("Failed to feed data to terminal: {}", e);
                }
            }
            PtyEvent::Exited(code) => {
                self.mark_exited(*code);
            }
            PtyEvent::Error(_) => {
                // Abnormal exit: mark exited but preserve any previously
                // recorded exit_code (matches pre-cas-e0b9 behavior).
                self.exited = true;
            }
        }
        Some(event)
    }

    pub fn drain_output(&mut self) -> (Vec<u8>, Vec<PtyEvent>) {
        let mut other_events = Vec::new();
        self.drain_buf.clear();

        let try_recv = |backend: &mut PaneBackend| -> Option<PtyEvent> {
            match backend {
                PaneBackend::Pty(pty) => pty.try_recv(),
                PaneBackend::None => None,
            }
        };

        while let Some(event) = try_recv(&mut self.backend) {
            match event {
                PtyEvent::Output(data) => {
                    self.drain_buf.extend_from_slice(&data);
                }
                PtyEvent::Exited(code) => {
                    self.mark_exited(code);
                    other_events.push(PtyEvent::Exited(code));
                }
                PtyEvent::Error(e) => {
                    // Abnormal exit: mark exited but preserve any previously
                    // recorded exit_code (matches pre-cas-e0b9 behavior).
                    self.exited = true;
                    other_events.push(PtyEvent::Error(e));
                }
            }
        }

        // Take the buffer out to avoid borrow conflict with self.feed()
        let coalesced = std::mem::take(&mut self.drain_buf);

        if !coalesced.is_empty() {
            self.total_bytes_received += coalesced.len() as u64;
            let feed_data = Self::strip_literal_cursor_reports(&coalesced);
            if let Err(e) = self.feed(feed_data.as_ref()) {
                tracing::warn!(
                    "Failed to feed {} bytes to terminal: {}",
                    feed_data.len(),
                    e
                );
            }
        }

        // Authoritative harness completion (Grok events.jsonl turn_ended).
        // Never use PTY quiet timers — long MCP/tool waits stay in-flight.
        if self.is_turn_in_flight() {
            self.refresh_harness_turn_state();
        }

        // Return the coalesced data directly — no clone needed since take()
        // already moved ownership out. drain_buf capacity is donated to the
        // caller but re-grows cheaply on the next cycle.
        (coalesced, other_events)
    }

    pub async fn write(&self, data: &[u8]) -> Result<()> {
        match &self.backend {
            PaneBackend::Pty(pty) => {
                pty.write(data).await?;
                Ok(())
            }
            PaneBackend::None => Err(Error::pty("Pane has no backend")),
        }
    }

    pub async fn send_line(&self, line: &str) -> Result<()> {
        match &self.backend {
            PaneBackend::Pty(pty) => {
                pty.send_line(line).await?;
                Ok(())
            }
            PaneBackend::None => Err(Error::pty("Pane has no backend")),
        }
    }

    /// Total bytes of PTY output observed for this pane since creation.
    ///
    /// Monotonically increasing; only advances when [`Pane::drain_output`]
    /// runs (i.e. when the daemon polls the mux). Used by the urgent
    /// interrupt-and-redirect path to detect output quiescence — a pane that
    /// has stopped emitting bytes after an interrupt has (best-effort) returned
    /// to its prompt and is safe to inject into.
    pub fn bytes_received(&self) -> u64 {
        self.total_bytes_received
    }

    /// Whether the pane's tracked turn is still in-flight (cas-7f6f).
    ///
    /// This is authoritative across normal completion only for Grok, whose
    /// `events.jsonl` supplies `turn_ended`. For Claude and Codex, `true`
    /// persists after normal completion until an explicit cancel/clear, and
    /// therefore MUST NOT be interpreted as "the harness is currently busy."
    pub fn is_turn_in_flight(&self) -> bool {
        self.turn_in_flight
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Set the harness session id (from `CAS_SESSION_ID` at spawn).
    pub fn set_harness_session_id(&self, session_id: impl Into<String>) {
        if let Ok(mut guard) = self.harness_session_id.lock() {
            *guard = Some(session_id.into());
        }
    }

    /// Harness session id, if known.
    pub fn harness_session_id(&self) -> Option<String> {
        self.harness_session_id.lock().ok().and_then(|g| g.clone())
    }

    /// Override the harness events path (tests only).
    pub fn set_harness_events_path_for_test(&self, path: impl Into<PathBuf>) {
        if let Ok(mut guard) = self.harness_events_path_override.lock() {
            *guard = Some(path.into());
        }
    }

    /// Mark that a turn has started (true prompt submit or inject).
    ///
    /// Snapshots the harness events file length so only later `turn_ended`
    /// events can complete this turn. Only Grok currently supplies that
    /// normal-completion signal; Claude/Codex remain set until explicit clear.
    pub fn mark_turn_in_flight(&self) {
        self.turn_in_flight
            .store(true, std::sync::atomic::Ordering::Release);
        let offset = self.harness_events_len().unwrap_or(0);
        if let Ok(mut guard) = self.turn_events_byte_offset.lock() {
            *guard = Some(offset);
        }
    }

    /// Explicitly clear the tracked turn (cancel or caller-known completion).
    ///
    /// Do not infer Claude/Codex completion from PTY quiet; long tool calls can
    /// be silent. No automatic normal-completion path exists for those harnesses.
    pub fn clear_turn_in_flight(&self) {
        self.turn_in_flight
            .store(false, std::sync::atomic::Ordering::Release);
        if let Ok(mut guard) = self.turn_events_byte_offset.lock() {
            *guard = None;
        }
    }

    /// Record normal completion reported by an authoritative external signal.
    ///
    /// The built-in caller is Grok `events.jsonl`. Claude/Codex currently have
    /// no such caller or signal.
    pub fn mark_turn_completed(&self) {
        self.clear_turn_in_flight();
    }

    /// Refresh in-flight state from the harness's authoritative turn signal.
    ///
    /// For Grok: reads `events.jsonl` after the offset captured at submit and
    /// clears on any `turn_ended` (outcomes completed|error|cancelled).
    /// Claude/Codex are deliberately unchanged: their tracked boolean is not a
    /// normal-completion/busy signal.
    /// Quiet PTY output alone never clears — long MCP/tool waits stay active.
    pub fn refresh_harness_turn_state(&self) {
        if !self.is_turn_in_flight() {
            return;
        }
        // Only Grok has an on-disk turn_ended signal we consume here.
        // Claude/Codex clear only on cancel or a caller-provided authoritative
        // completion; no such normal-completion caller currently exists.
        if self.harness != SupervisorCli::Grok
            && self
                .harness_events_path_override
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .is_none()
        {
            return;
        }
        let Some(path) = self.resolve_harness_events_path() else {
            return;
        };
        let start = self
            .turn_events_byte_offset
            .lock()
            .ok()
            .and_then(|g| *g)
            .unwrap_or(0);
        let Ok(data) = std::fs::read(&path) else {
            return;
        };
        if (data.len() as u64) <= start {
            return;
        }
        let suffix = &data[start as usize..];
        if harness_events_suffix_has_turn_ended(suffix) {
            self.mark_turn_completed();
        }
    }

    fn resolve_harness_events_path(&self) -> Option<PathBuf> {
        if let Ok(guard) = self.harness_events_path_override.lock() {
            if let Some(ref p) = *guard {
                return Some(p.clone());
            }
        }
        let sid = self.harness_session_id()?;
        find_grok_events_jsonl(&sid)
    }

    fn harness_events_len(&self) -> Option<u64> {
        let path = self.resolve_harness_events_path()?;
        std::fs::metadata(path).ok().map(|m| m.len())
    }

    /// Whether this pane is ready to accept prompt injection.
    /// Claude Code flushes the PTY input buffer during startup, so text
    /// written before readline initialization is silently lost. We require
    /// both output (Claude has booted) AND a 5-second grace period.
    pub fn ready_for_injection(&self) -> bool {
        self.total_bytes_received > 0
            && self.created_at.elapsed() >= std::time::Duration::from_secs(5)
    }

    /// Type `prompt` into the pane's harness and submit it as a fresh turn.
    ///
    /// The payload is framed per harness (cas-5fff): Codex requires explicit
    /// bracketed-paste delimiters, without which its paste-burst detector
    /// consumes the trailing CR as the paste terminator and leaves the whole
    /// message sitting in the composer as an unsubmitted draft — see
    /// [`crate::harness::HarnessCapabilities::requires_bracketed_paste_injection`]
    /// for the live measurements. Claude and Grok payloads are unchanged.
    ///
    /// This is the single choke point for BOTH injection paths — normal
    /// (`Mux::inject`) and urgent (`Mux::interrupt_and_inject`, which calls
    /// this after `break_turn`) — so the framing contract cannot drift between
    /// them. That is also why the cas-5fff defect degraded urgent delivery
    /// into "usually, retry if not" rather than leaving it healthy: both paths
    /// were sampling the same swallow, with payload size as the variable.
    pub async fn inject_prompt(&self, prompt: &str) -> Result<()> {
        match &self.backend {
            PaneBackend::Pty(pty) => {
                let text = prompt.trim();
                let framed = crate::harness::injection_payload_bytes(self.harness, text);
                pty.write(&framed).await?;
                // Send carriage return after a settle delay in a background task
                // so we don't block the daemon event loop for 150-500ms.
                let writer = pty.writer_handle();
                let settle_ms = if pty.is_codex() { 500 } else { 150 };
                // cas-ae6d (GH #100): the submit CR is what turns injected text
                // into an actual turn. It is written from a detached task, so
                // its failure cannot be returned to the caller — which already
                // recorded the inject as Delivered and marked the pane
                // turn-in-flight. Swallowing the error silently leaves the
                // payload sitting in the harness composer as an unsubmitted
                // draft while every observer believes the agent was woken (the
                // "assignment delivered, worker still idle" shape). It stays
                // detached (blocking the daemon loop 500ms per inject is worse),
                // but the failure is now visible on the same
                // `cas::coordination` lane the delivery telemetry uses.
                let pane_id = self.id.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(settle_ms)).await;
                    let mut guard = writer.lock().await;
                    if let Err(e) = guard.write_all(b"\r").and_then(|()| guard.flush()) {
                        tracing::warn!(
                            target: "cas::coordination",
                            stage = "inject_submit_failed",
                            target_agent = %pane_id,
                            error = %e,
                            "injected payload was written but its submit CR failed — the text is \
                             sitting unsubmitted in the pane composer and no turn started"
                        );
                    }
                });
                self.clear_composer_dirty();
                // Inject submits a prompt → turn is in flight (cas-7f6f).
                self.mark_turn_in_flight();
                Ok(())
            }
            PaneBackend::None => Err(Error::pty("Pane has no backend")),
        }
    }

    /// Whether `data` is a true keyboard prompt submit (lone CR/LF), not a
    /// multi-byte paste/drop payload that happens to embed newlines.
    pub fn is_true_prompt_submit(data: &[u8]) -> bool {
        matches!(data, b"\r" | b"\n" | b"\r\n")
    }

    /// Whether a [`UserInputKind::KeyStream`] chunk should mark turn-in-flight.
    ///
    /// Uses a **fresh** bracketed-paste tracker over the whole chunk (cas-4b99):
    /// embedded CR/LF inside `\x1b[200~`…`\x1b[201~` do not mark submit; CR/LF
    /// outside paste delimiters do. For streaming / split packets prefer
    /// [`Self::deliver_user_input`], which keeps pane-local tracker state.
    ///
    /// Structured paste/drop should still use [`UserInputKind::StructuredPaste`]
    /// — that path never marks submit and does not advance the stream tracker.
    pub fn key_stream_is_submit(data: &[u8]) -> bool {
        crate::input_stream::key_stream_marks_submit(data)
    }

    /// Deliver user input with explicit submit semantics (cas-7f6f / cas-4b99).
    ///
    /// - [`UserInputKind::KeyStream`]: advances the pane's bracketed-paste
    ///   tracker and marks turn-in-flight only on CR/LF outside paste.
    /// - [`UserInputKind::StructuredPaste`]: never marks (paste/drop); does not
    ///   advance the KeyStream paste tracker (payload is already framed).
    pub async fn deliver_user_input(&self, data: &[u8], kind: UserInputKind) -> Result<()> {
        let marks_submit = match kind {
            UserInputKind::KeyStream => self.observe_key_stream_composer_input(data),
            UserInputKind::StructuredPaste => {
                if !data.is_empty() {
                    self.mark_composer_dirty();
                }
                false
            }
        };
        if marks_submit {
            self.mark_turn_in_flight();
        }
        self.write(data).await
    }

    /// Whether an attached operator currently has an unsubmitted draft.
    pub fn is_composer_dirty(&self) -> bool {
        self.composer_dirty
            .load(std::sync::atomic::Ordering::Acquire)
    }

    fn mark_composer_dirty(&self) {
        if !self
            .composer_dirty
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            match self.composer_dirty_since.lock() {
                Ok(mut since) => *since = Some(std::time::Instant::now()),
                Err(poisoned) => {
                    *poisoned.into_inner() = Some(std::time::Instant::now());
                }
            }
        }
    }

    fn clear_composer_dirty(&self) {
        self.composer_dirty
            .store(false, std::sync::atomic::Ordering::Release);
        match self.composer_dirty_since.lock() {
            Ok(mut since) => *since = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
    }

    pub(crate) fn composer_dirty_elapsed(&self) -> Option<std::time::Duration> {
        let since = match self.composer_dirty_since.lock() {
            Ok(since) => *since,
            Err(poisoned) => *poisoned.into_inner(),
        };
        since.map(|started| started.elapsed())
    }

    /// Track the final composer state after one attached-client keystream
    /// chunk. The same streaming paste tracker that recognizes true submits
    /// also keeps embedded paste newlines from looking like Enter.
    fn observe_key_stream_composer_input(&self, data: &[u8]) -> bool {
        let mut dirty = self.is_composer_dirty();
        let mut marks_submit = false;
        let mut tracker = match self.paste_tracker.lock() {
            Ok(tracker) => tracker,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut escape_state = ComposerEscapeState::Normal;

        for &byte in data {
            let class = tracker.feed_byte(byte);
            if class == crate::input_stream::StreamByteClass::Paste {
                dirty = true;
                escape_state = ComposerEscapeState::Normal;
                continue;
            }

            match escape_state {
                ComposerEscapeState::Escape => match byte {
                    b'[' | b'O' => {
                        escape_state = ComposerEscapeState::ControlSequence;
                        continue;
                    }
                    0x1b => {
                        dirty = false;
                        continue;
                    }
                    _ => {
                        // The preceding Esc was standalone. Apply its clear
                        // before classifying the current non-sequence byte.
                        dirty = false;
                        escape_state = ComposerEscapeState::Normal;
                    }
                },
                ComposerEscapeState::ControlSequence => {
                    if (0x40..=0x7e).contains(&byte) {
                        escape_state = ComposerEscapeState::Normal;
                    }
                    continue;
                }
                ComposerEscapeState::Normal => {}
            }

            match byte {
                b'\r' | b'\n' => {
                    dirty = false;
                    marks_submit = true;
                }
                // Defer deciding whether Esc is standalone until the next
                // byte so a complete CSI/SS3 sequence does not clear or dirty
                // the composer.
                0x1b => escape_state = ComposerEscapeState::Escape,
                // The two common line-clear/cancel controls.
                0x03 | 0x15 => dirty = false,
                // Printable input plus editing keys that can create/modify an
                // unsubmitted draft. Backspace stays conservatively sticky:
                // without the inner composer's buffer we cannot know whether
                // it erased the final character.
                b'\t' | 0x7f | 0x20..=0x7e => dirty = true,
                _ => {}
            }
        }
        if escape_state == ComposerEscapeState::Escape {
            dirty = false;
        }
        if dirty {
            self.mark_composer_dirty();
        } else {
            self.clear_composer_dirty();
        }
        marks_submit
    }

    /// Observe a raw control write from an attached client. Raw writes are
    /// used for standalone Esc and navigation/SGR sequences; only exact clear
    /// controls reset state so arrow/mouse escape sequences do not.
    pub(crate) fn observe_raw_client_input(&self, data: &[u8]) {
        if matches!(data, b"\x1b" | b"\x03" | b"\x15") {
            self.clear_composer_dirty();
        }
    }

    /// Break the current turn with a harness-aware cancel payload (cas-7f6f).
    ///
    /// - **Claude / Codex**: Esc (`0x1b`) — Claude Code's cancel-turn key
    ///   (Ctrl+C is the double-press quit signal).
    /// - **Grok**: Ctrl+C (`0x03`) — since 0.2.93 Esc is a mid-turn no-op;
    ///   cancel is Ctrl+C (see [`SupervisorCli::turn_cancel_bytes`]).
    ///
    /// Used by the urgent interrupt-and-redirect path and by factory Escape
    /// routing so UI cancel and programmatic cancel share one tested path.
    /// Clears [`Self::is_turn_in_flight`] after issuing cancel (Grok re-cancel
    /// while cancelling uses raw Esc, which is correct for that state).
    pub async fn break_turn(&self) -> Result<()> {
        match &self.backend {
            PaneBackend::Pty(pty) => {
                pty.write(self.harness.turn_cancel_bytes()).await?;
                self.clear_turn_in_flight();
                self.clear_composer_dirty();
                Ok(())
            }
            PaneBackend::None => Err(Error::pty("Pane has no backend")),
        }
    }

    pub async fn interrupt(&self) -> Result<()> {
        match &self.backend {
            PaneBackend::Pty(pty) => {
                pty.interrupt().await?;
                // Ctrl+C is also a cancel path for Grok mid-turn.
                self.clear_turn_in_flight();
                self.clear_composer_dirty();
                Ok(())
            }
            PaneBackend::None => Err(Error::pty("Pane has no backend")),
        }
    }

    /// Interactive harness for this pane (Claude / Codex / Grok).
    pub fn harness(&self) -> SupervisorCli {
        self.harness
    }

    /// Override the harness (tests and late-bound config only).
    pub fn set_harness(&mut self, harness: SupervisorCli) {
        self.harness = harness;
    }

    pub fn scroll(&mut self, delta: i32) -> Result<()> {
        let info_before = self.terminal.scrollback_info();
        if debug_log_enabled() {
            tracing::debug!(
                "Pane {}: scroll delta={}, before: offset={}, total={}",
                self.id,
                delta,
                info_before.viewport_offset,
                info_before.total_scrollback
            );
        }
        let result = self
            .terminal
            .scroll(delta)
            .map_err(|e| Error::terminal(e.to_string()));
        let info_after = self.terminal.scrollback_info();

        // Track whether user has scrolled away from bottom
        if info_after.viewport_offset > 0 {
            self.user_scrolled = true;
        } else {
            self.user_scrolled = false;
            self.new_lines_below = 0;
        }

        if debug_log_enabled() {
            tracing::debug!(
                "Pane {}: scroll complete, after: offset={}, total={}",
                self.id,
                info_after.viewport_offset,
                info_after.total_scrollback
            );
        }
        result
    }

    pub fn scroll_to_top(&mut self) -> Result<()> {
        self.terminal
            .scroll_to_top()
            .map_err(|e| Error::terminal(e.to_string()))
    }

    pub fn scroll_to_bottom(&mut self) -> Result<()> {
        self.user_scrolled = false;
        self.new_lines_below = 0;
        self.terminal
            .scroll_to_bottom()
            .map_err(|e| Error::terminal(e.to_string()))
    }

    /// Whether the user has scrolled up from the bottom
    pub fn is_user_scrolled(&self) -> bool {
        self.user_scrolled
    }

    /// Number of new output lines received while user was scrolled up
    pub fn new_lines_below(&self) -> u32 {
        self.new_lines_below
    }

    pub fn kill(&mut self) {
        match &mut self.backend {
            PaneBackend::Pty(pty) => pty.kill(),
            PaneBackend::None => {}
        }
    }

    /// Process group owned by this pane's PTY child.
    pub fn process_group_id(&self) -> Option<u32> {
        match &self.backend {
            PaneBackend::Pty(pty) => pty.process_group_id(),
            PaneBackend::None => None,
        }
    }

    /// Kill the pane's process tree (cas-8c5a).
    ///
    /// Delegates to `Pty::kill_tree(force)`, including its async grace window.
    pub async fn kill_tree(&mut self, force: bool) {
        match &mut self.backend {
            PaneBackend::Pty(pty) => pty.kill_tree(force).await,
            PaneBackend::None => {}
        }
    }

    /// Immediate teardown used when the whole factory process is exiting.
    pub fn kill_tree_force(&mut self) {
        match &mut self.backend {
            PaneBackend::Pty(pty) => pty.kill_tree_force(),
            PaneBackend::None => {}
        }
    }

    pub async fn start_recording(
        &mut self,
        session_id: impl Into<String>,
        config: WriterConfig,
    ) -> Result<()> {
        if self.recorder.is_some() {
            return Err(Error::recording("Recording already in progress"));
        }

        let writer = RecordingWriter::new(
            self.cols,
            self.rows,
            self.id.clone(),
            session_id.into(),
            self.kind.as_str(),
            config,
        )
        .await
        .map_err(|e| Error::recording(e.to_string()))?;

        self.recorder = Some(Arc::new(Mutex::new(writer)));

        self.generate_keyframe().await?;

        tracing::info!("Started recording for pane {}", self.id);
        Ok(())
    }

    pub async fn stop_recording(&mut self) -> Result<Option<PathBuf>> {
        if let Some(recorder) = self.recorder.take() {
            let writer = match Arc::try_unwrap(recorder) {
                Ok(mutex) => mutex.into_inner(),
                Err(_) => return Err(Error::recording("Recording still in use")),
            };
            let path = writer.file_path().clone();
            writer
                .close()
                .await
                .map_err(|e| Error::recording(e.to_string()))?;
            tracing::info!(
                "Stopped recording for pane {}, saved to {:?}",
                self.id,
                path
            );
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    async fn generate_keyframe(&mut self) -> Result<()> {
        if let Some(ref recorder) = self.recorder {
            let mut lines = Vec::new();
            for row in 0..self.rows {
                let text = self
                    .terminal
                    .dump_screen_row(row as u32)
                    .unwrap_or_default();
                lines.push(text);
            }
            let content = lines.join("\n").into_bytes();

            let mut writer = recorder.lock().await;
            writer
                .write_keyframe(content)
                .await
                .map_err(|e| Error::recording(e.to_string()))?;
        }
        Ok(())
    }

    pub async fn record_output(&mut self, data: &[u8]) -> Result<()> {
        if let Some(ref recorder) = self.recorder {
            let writer = recorder.lock().await;
            writer
                .write_output(data)
                .await
                .map_err(|e| Error::recording(e.to_string()))?;
        }
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }
}

fn cas_session_id_from_config(config: &PtyConfig) -> Option<String> {
    config
        .env
        .iter()
        .find(|(k, _)| k == "CAS_SESSION_ID")
        .map(|(_, v)| v.clone())
}

/// Grok sessions root: `$GROK_HOME/sessions` or `~/.grok/sessions`.
fn grok_sessions_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("GROK_HOME") {
        return Some(PathBuf::from(home).join("sessions"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".grok").join("sessions"))
}

/// Locate `~/.grok/sessions/*/<session_id>/events.jsonl`.
fn find_grok_events_jsonl(session_id: &str) -> Option<PathBuf> {
    let sessions = grok_sessions_dir()?;
    let entries = std::fs::read_dir(sessions).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(session_id).join("events.jsonl");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Whether a suffix of `events.jsonl` contains a `turn_ended` event.
fn harness_events_suffix_has_turn_ended(suffix: &[u8]) -> bool {
    for line in suffix.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("turn_ended") {
            return true;
        }
    }
    false
}

pub(crate) fn push_factory_session_env(
    config: &mut PtyConfig,
    cli: SupervisorCli,
    factory_session: Option<&str>,
) {
    if let Some(session) = factory_session {
        config
            .env
            .push(("CAS_FACTORY_SESSION".to_string(), session.to_string()));
        // cas-6569 (EPIC cas-8888, Phase 2) SILENT SITE — audited and
        // RESOLVED, same rationale as build_worker_config above: Grok has
        // no `-c`-style flag to mirror, and gets CAS_FACTORY_SESSION via
        // the plain env var pushed just above (process env inheritance),
        // same as Claude.
        if cli == SupervisorCli::Codex {
            let session = sanitize_factory_session_for_toml_arg(session);
            config.args.push("-c".to_string());
            config.args.push(format!(
                "mcp_servers.cs.env.CAS_FACTORY_SESSION=\"{session}\""
            ));
        }
    }
}

fn sanitize_factory_session_for_toml_arg(session: &str) -> String {
    session
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod hyperlink_gate_tests {
    use super::Pane;

    #[test]
    fn link_free_panes_skip_row_hyperlink_scan() {
        let mut pane = Pane::director("plain", 2, 10).unwrap();
        pane.feed(b"plain text").unwrap();

        assert!(!pane.has_hyperlinks());
        assert!(pane.row_hyperlinks(0).is_empty());
    }

    #[test]
    fn split_osc8_introducer_enables_row_hyperlink_scan() {
        let mut pane = Pane::director("linked", 1, 20).unwrap();
        pane.feed(b"\x1b]").unwrap();
        assert!(!pane.has_hyperlinks());

        pane.feed(b"8;;https://split.example\x1b\\x\x1b]8;;\x1b\\")
            .unwrap();

        assert!(pane.has_hyperlinks());
        let row_links = pane.row_hyperlinks(0);
        assert_eq!(
            row_links.first().and_then(Option::as_deref),
            Some("https://split.example")
        );
    }
}
