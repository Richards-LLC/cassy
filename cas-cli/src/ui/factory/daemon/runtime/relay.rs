use std::collections::{HashMap, VecDeque};
use std::fmt::Write as FmtWrite;

use cas_factory_protocol::{
    STYLE_BOLD, STYLE_FAINT, STYLE_INVERSE, STYLE_INVISIBLE, STYLE_ITALIC, STYLE_STRIKETHROUGH,
    STYLE_UNDERLINE, TerminalSnapshot,
};

use crate::ui::factory::daemon::FactoryDaemon;
use crate::ui::factory::daemon::cloud_client::RelayEvent;

/// Max bytes to keep per pane for replay on attach (256 KB).
const PANE_BUFFER_CAPACITY: usize = 256 * 1024;

/// Tracks a remote relay client connected via cloud WebSocket
#[derive(Debug)]
pub struct RelayClient {
    /// Terminal columns
    pub cols: u16,
    /// Terminal rows
    pub rows: u16,
    /// Whether this client can send input (vs read-only)
    pub interactive: bool,
}

/// Ring buffer of raw PTY bytes for a single pane.
///
/// When a web viewer attaches, we replay this buffer so xterm.js
/// processes the same byte stream and ends up in the identical
/// terminal state (alternate screen, colors, cursor, etc.).
#[derive(Debug, Default)]
pub struct PaneBuffer {
    data: VecDeque<u8>,
}

impl PaneBuffer {
    pub(in crate::ui::factory::daemon) fn append(&mut self, bytes: &[u8]) {
        self.data.extend(bytes);
        // Trim from the front if over capacity
        if self.data.len() > PANE_BUFFER_CAPACITY {
            let excess = self.data.len() - PANE_BUFFER_CAPACITY;
            self.data.drain(..excess);
        }
    }

    pub(in crate::ui::factory::daemon) fn as_bytes(&self) -> Vec<u8> {
        let (front, back) = self.data.as_slices();
        let mut v = Vec::with_capacity(front.len() + back.len());
        v.extend_from_slice(front);
        v.extend_from_slice(back);
        v
    }

    pub(in crate::ui::factory::daemon) fn replace_with(&mut self, bytes: Vec<u8>) {
        self.data.clear();
        self.data.extend(bytes);
        if self.data.len() > PANE_BUFFER_CAPACITY {
            let excess = self.data.len() - PANE_BUFFER_CAPACITY;
            self.data.drain(..excess);
        }
    }

    /// Return the buffer contents as plain text with all ANSI escape sequences stripped.
    ///
    /// Handles CSI sequences (ESC[...X), OSC sequences (ESC]...ST/BEL),
    /// and two-byte ESC+char sequences.
    #[allow(dead_code)]
    pub fn as_plain_text(&self) -> String {
        let bytes = self.as_bytes();
        strip_ansi(&bytes)
    }
}

/// Strip ANSI escape sequences from raw PTY bytes, returning plain UTF-8 text.
///
/// Handles:
/// - CSI sequences: ESC [ ... (final byte 0x40–0x7E)
/// - OSC sequences: ESC ] ... (terminated by BEL or ST)
/// - Two-byte ESC sequences: ESC + single byte
/// - Bare CR (\r) is consumed (line content replaces itself)
fn strip_ansi(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b => {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                match bytes[i] {
                    b'[' => {
                        // CSI sequence: consume until final byte (0x40–0x7E)
                        i += 1;
                        while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                            i += 1;
                        }
                        if i < bytes.len() {
                            i += 1; // consume final byte
                        }
                    }
                    b']' => {
                        // OSC sequence: consume until BEL (0x07) or ST (ESC \)
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if bytes[i] == 0x1b
                                && i + 1 < bytes.len()
                                && bytes[i + 1] == b'\\'
                            {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    _ => {
                        // Two-byte escape (e.g. ESC M, ESC 7, ESC 8)
                        i += 1;
                    }
                }
            }
            b'\r' => {
                // Carriage return — skip (newlines are \n)
                i += 1;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Interval between pane-tail snapshot writes.
#[allow(dead_code)]
const SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Write ANSI-stripped plain text snapshots to disk for each pane.
///
/// Files are written atomically (write to .tmp, then rename) to
/// `~/.cas/sessions/{session_name}/pane-tail/{pane_id}.txt`.
#[allow(dead_code)]
pub(in crate::ui::factory::daemon) fn write_pane_snapshots(
    session_name: &str,
    pane_buffers: &HashMap<String, PaneBuffer>,
    last_snapshot_at: &mut HashMap<String, std::time::Instant>,
) {
    let now = std::time::Instant::now();

    // Only proceed if at least one pane is due for a snapshot
    let any_due = pane_buffers.keys().any(|id| {
        last_snapshot_at
            .get(id)
            .map(|t| now.duration_since(*t) >= SNAPSHOT_INTERVAL)
            .unwrap_or(true)
    });
    if !any_due {
        return;
    }

    let tail_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".cas")
        .join("sessions")
        .join(session_name)
        .join("pane-tail");

    // Create dir once (idempotent)
    if let Err(e) = std::fs::create_dir_all(&tail_dir) {
        tracing::warn!("Failed to create pane-tail dir: {e}");
        return;
    }

    for (pane_id, buffer) in pane_buffers {
        let due = last_snapshot_at
            .get(pane_id)
            .map(|t| now.duration_since(*t) >= SNAPSHOT_INTERVAL)
            .unwrap_or(true);
        if !due {
            continue;
        }

        let text = buffer.as_plain_text();
        let dest = tail_dir.join(format!("{pane_id}.txt"));
        let tmp = tail_dir.join(format!("{pane_id}.txt.tmp"));

        match std::fs::write(&tmp, text.as_bytes()) {
            Ok(()) => {
                if let Err(e) = std::fs::rename(&tmp, &dest) {
                    tracing::warn!("Failed to rename pane-tail snapshot for {pane_id}: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("Failed to write pane-tail snapshot for {pane_id}: {e}");
            }
        }

        last_snapshot_at.insert(pane_id.clone(), now);
    }
}

/// Render a `TerminalSnapshot` as raw ANSI escape sequences.
///
/// After a PTY resize the virtual terminal reflows content to the new
/// dimensions. This function captures that reflowed viewport as ANSI bytes
/// suitable for replaying into a web terminal emulator (ghostty-web / xterm.js),
/// preserving colors, bold/italic, and cursor position.
fn snapshot_to_ansi(snap: &TerminalSnapshot, in_alt_screen: bool) -> Vec<u8> {
    let cap = (snap.rows as usize) * (snap.cols as usize) * 4;
    let mut buf = Vec::with_capacity(cap);
    let mut tmp = String::new();

    // The snapshot is taken from the currently visible buffer.  When that is
    // an alternate screen, enter it before replaying the rows: a live Ink UI
    // subsequently sends cursor-addressed incremental redraws and assumes
    // that its alternate buffer is still selected.
    if in_alt_screen {
        buf.extend_from_slice(b"\x1b[?1049h");
    }

    // Reset attributes, clear screen, cursor home.
    buf.extend_from_slice(b"\x1b[0m\x1b[2J\x1b[H");

    let mut prev_fg: (u8, u8, u8) = (0, 0, 0);
    let mut prev_bg: (u8, u8, u8) = (0, 0, 0);
    let mut prev_flags: u32 = 0;

    for row in 0..snap.rows {
        // Move cursor to start of row
        tmp.clear();
        let _ = write!(tmp, "\x1b[{};1H", row + 1);
        buf.extend_from_slice(tmp.as_bytes());

        for col in 0..snap.cols {
            let idx = (row as usize) * (snap.cols as usize) + (col as usize);
            let cell = &snap.cells[idx];

            // Emit SGR sequence when style changes
            if cell.fg != prev_fg || cell.bg != prev_bg || cell.flags != prev_flags {
                emit_sgr(&mut buf, cell.fg, cell.bg, cell.flags);
                prev_fg = cell.fg;
                prev_bg = cell.bg;
                prev_flags = cell.flags;
            }

            // Write character
            let ch = char::from_u32(cell.codepoint).unwrap_or(' ');
            let mut char_buf = [0u8; 4];
            buf.extend_from_slice(ch.encode_utf8(&mut char_buf).as_bytes());
        }
    }

    // Reset attributes, then position cursor
    buf.extend_from_slice(b"\x1b[0m");
    tmp.clear();
    let _ = write!(tmp, "\x1b[{};{}H", snap.cursor.y + 1, snap.cursor.x + 1);
    buf.extend_from_slice(tmp.as_bytes());

    buf
}

/// Emit an SGR (Select Graphic Rendition) escape sequence into `buf`.
fn emit_sgr(buf: &mut Vec<u8>, fg: (u8, u8, u8), bg: (u8, u8, u8), flags: u32) {
    // Start with reset so each SGR is self-contained
    let mut params = String::from("\x1b[0");

    if flags & STYLE_BOLD != 0 {
        params.push_str(";1");
    }
    if flags & STYLE_FAINT != 0 {
        params.push_str(";2");
    }
    if flags & STYLE_ITALIC != 0 {
        params.push_str(";3");
    }
    if flags & STYLE_UNDERLINE != 0 {
        params.push_str(";4");
    }
    if flags & STYLE_INVERSE != 0 {
        params.push_str(";7");
    }
    if flags & STYLE_INVISIBLE != 0 {
        params.push_str(";8");
    }
    if flags & STYLE_STRIKETHROUGH != 0 {
        params.push_str(";9");
    }

    // 24-bit fg color (skip if default black)
    if fg != (0, 0, 0) {
        let _ = write!(params, ";38;2;{};{};{}", fg.0, fg.1, fg.2);
    }
    // 24-bit bg color (skip if default black)
    if bg != (0, 0, 0) {
        let _ = write!(params, ";48;2;{};{};{}", bg.0, bg.1, bg.2);
    }

    params.push('m');
    buf.extend_from_slice(params.as_bytes());
}

impl FactoryDaemon {
    /// Process pending relay events from the cloud client.
    ///
    /// Called each tick in the main daemon loop. Handles attach/detach/input/resize
    /// from remote users connected via the cloud relay channel.
    pub(super) async fn process_relay_events(&mut self) {
        let events = match self.cloud_handle {
            Some(ref handle) => handle.try_recv_relay(),
            None => return,
        };

        for event in events {
            match event {
                RelayEvent::AttachRequest {
                    client_id,
                    cols,
                    rows,
                    mode,
                } => {
                    self.handle_relay_attach(&client_id, cols, rows, &mode);
                }
                RelayEvent::PtyInput { client_id, data } => {
                    self.handle_relay_input(&client_id, &data).await;
                }
                RelayEvent::Resize {
                    client_id,
                    cols,
                    rows,
                } => {
                    self.handle_relay_resize(&client_id, cols, rows);
                }
                RelayEvent::Detach { client_id } => {
                    self.handle_relay_detach(&client_id);
                }
                RelayEvent::PaneAttach { pane, cols, rows } => {
                    self.handle_pane_attach(&pane, cols, rows);
                }
                RelayEvent::PaneResize { pane, cols, rows } => {
                    self.handle_pane_resize(&pane, cols, rows);
                }
                RelayEvent::PaneDetach { pane } => {
                    self.handle_pane_detach(&pane);
                }
                RelayEvent::PaneInput { pane, data } => {
                    self.handle_pane_input(&pane, &data).await;
                }
            }
        }
    }

    fn handle_relay_attach(&mut self, client_id: &str, cols: u16, rows: u16, mode: &str) {
        let interactive = mode != "readonly";

        let client = RelayClient {
            cols,
            rows,
            interactive,
        };

        tracing::info!(
            "Relay client {} attached ({}x{}, {})",
            client_id,
            cols,
            rows,
            if interactive {
                "interactive"
            } else {
                "readonly"
            }
        );

        self.relay_clients.insert(client_id.to_string(), client);

        // Accept the attach request
        if let Some(ref handle) = self.cloud_handle {
            handle.relay_accept(client_id, cols, rows);
        }
    }

    async fn handle_relay_input(&mut self, client_id: &str, data: &[u8]) {
        let is_interactive = self
            .relay_clients
            .get(client_id)
            .is_some_and(|c| c.interactive);

        if !is_interactive {
            return;
        }

        // Unified KeyStream submit API (cas-7f6f) — same as terminal/GUI/WS.
        let _ = self
            .app
            .mux
            .deliver_user_input(data, cas_mux::UserInputKind::KeyStream)
            .await;
    }

    fn handle_relay_resize(&mut self, client_id: &str, cols: u16, rows: u16) {
        if let Some(client) = self.relay_clients.get_mut(client_id) {
            client.cols = cols;
            client.rows = rows;
            tracing::debug!("Relay client {} resized to {}x{}", client_id, cols, rows);
        }
    }

    fn handle_relay_detach(&mut self, client_id: &str) {
        if self.relay_clients.remove(client_id).is_some() {
            tracing::info!("Relay client {} detached", client_id);
        }
    }

    /// Broadcast rendered TUI output to all relay clients.
    ///
    /// Called after rendering, sends the full terminal output frame to each
    /// relay client via the cloud WebSocket.
    pub(super) fn broadcast_relay_output(&self, output: &[u8]) {
        if self.relay_clients.is_empty() || output.is_empty() {
            return;
        }
        if let Some(ref handle) = self.cloud_handle {
            for client_id in self.relay_clients.keys() {
                handle.relay_output(client_id, output.to_vec());
            }
        }
    }

    /// Check if any relay clients are connected
    pub(super) fn has_relay_clients(&self) -> bool {
        !self.relay_clients.is_empty()
    }

    // --- Per-pane relay (web terminal viewers) ---

    /// Buffer raw PTY bytes for a pane. Called for EVERY PaneOutput event,
    /// regardless of whether anyone is watching. This ensures the buffer
    /// is warm when a viewer attaches.
    pub(super) fn buffer_pane_output(&mut self, pane_id: &str, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.pane_buffers
            .entry(pane_id.to_string())
            .or_default()
            .append(data);
    }

    /// Rebuild all pane ring buffers from current terminal snapshots.
    ///
    /// Called after a PTY resize so the buffers contain ANSI bytes
    /// rendered at the new dimensions (the virtual terminal reflows
    /// content on resize). Without this, the buffer would hold a mix
    /// of old-size and new-size bytes which looks garbled.
    pub(super) fn rebuild_pane_buffers_from_snapshots(&mut self) {
        let pane_ids: Vec<String> = self.pane_buffers.keys().cloned().collect();
        for pane_id in pane_ids {
            if let Some(pane) = self.app.mux.get(&pane_id) {
                if let Ok(snapshot) = pane.get_full_snapshot() {
                    let ansi = snapshot_to_ansi(&snapshot, pane.is_in_alt_screen());
                    if let Some(buffer) = self.pane_buffers.get_mut(&pane_id) {
                        buffer.replace_with(ansi);
                    }
                }
            }
        }
    }

    fn handle_pane_attach(&mut self, pane: &str, cols: Option<u16>, rows: Option<u16>) {
        let watchers = self.pane_watchers.entry(pane.to_string()).or_default();
        let was_empty = watchers.is_empty();
        watchers.insert("web".to_string());

        if was_empty {
            tracing::info!(
                "Pane '{}' now has web watchers, starting output relay",
                pane
            );

            // Send the pane list so the frontend knows available panes
            if let Some(ref handle) = self.cloud_handle {
                let mut panes = self.app.worker_names().to_vec();
                panes.insert(0, self.app.supervisor_name().to_string());
                handle.send_pane_list(panes);
            }
        }

        let actual_pane = if pane == "supervisor" {
            self.app.supervisor_name().to_string()
        } else {
            pane.to_string()
        };

        // If the web viewer sent dimensions, resize the PTY to match so
        // output is formatted for the viewer's terminal size.
        if let (Some(c), Some(r)) = (cols, rows) {
            if let Some(mux_pane) = self.app.mux.get_mut(&actual_pane) {
                let (old_rows, old_cols) = mux_pane.size();
                if old_cols != c || old_rows != r {
                    match mux_pane.resize(r, c) {
                        Ok(()) => {
                            tracing::info!(
                                "Resized pane '{}' from {}x{} to {}x{} for web viewer",
                                pane,
                                old_cols,
                                old_rows,
                                c,
                                r
                            );
                            // Rebuild the buffer from the vt snapshot at the
                            // new dimensions (the virtual terminal reflows on
                            // resize so the snapshot has correct content).
                            if let Some(p) = self.app.mux.get(&actual_pane) {
                                if let Ok(snapshot) = p.get_full_snapshot() {
                                    let ansi = snapshot_to_ansi(&snapshot, p.is_in_alt_screen());
                                    self.pane_buffers
                                        .entry(actual_pane.clone())
                                        .or_default()
                                        .replace_with(ansi);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to resize pane '{}': {}", pane, e);
                        }
                    }
                }
            }
        }

        // Replay buffered PTY bytes so the browser's terminal emulator ends up
        // in the same terminal state (alternate screen, colors, cursor, etc.).
        if let Some(buffer) = self.pane_buffers.get(&actual_pane) {
            let replay = buffer.as_bytes();
            if !replay.is_empty() {
                tracing::info!(
                    "Replaying {} bytes of PTY history for pane '{}'",
                    replay.len(),
                    pane
                );
                if let Some(ref handle) = self.cloud_handle {
                    handle.send_pane_output(pane, replay);
                }
            }
        }
    }

    fn handle_pane_resize(&mut self, pane: &str, cols: u16, rows: u16) {
        let actual_pane = if pane == "supervisor" {
            self.app.supervisor_name().to_string()
        } else {
            pane.to_string()
        };

        // Store web viewer's size and apply unified effective minimum
        self.web_pane_sizes
            .insert(actual_pane.clone(), (cols, rows));
        self.apply_effective_pane_size(&actual_pane);

        // Rebuild the buffer from the reflowed vt snapshot
        if let Some(p) = self.app.mux.get(&actual_pane) {
            if let Ok(snapshot) = p.get_full_snapshot() {
                let ansi = snapshot_to_ansi(&snapshot, p.is_in_alt_screen());
                self.pane_buffers
                    .entry(actual_pane)
                    .or_default()
                    .replace_with(ansi);
            }
        }
    }

    fn handle_pane_detach(&mut self, pane: &str) {
        let actual_pane = if pane == "supervisor" {
            self.app.supervisor_name().to_string()
        } else {
            pane.to_string()
        };

        if let Some(watchers) = self.pane_watchers.get_mut(pane) {
            watchers.remove("web");
            if watchers.is_empty() {
                self.pane_watchers.remove(pane);
                self.web_pane_sizes.remove(&actual_pane);
                tracing::info!("Pane '{}' has no more web watchers", pane);
            }
        }

        // When no web viewers remain on ANY pane, clear all web sizes
        // and restore panes to their effective size (TUI/GUI constraints).
        if self.pane_watchers.is_empty() {
            self.web_pane_sizes.clear();
            if self.cols > 0 && self.rows > 0 {
                tracing::info!(
                    "No web viewers remain, restoring local terminal size {}x{}",
                    self.cols,
                    self.rows
                );
                let _ = self.app.handle_resize(self.cols, self.rows);
                self.snapshot_tui_pane_sizes_and_reconcile();
                self.rebuild_pane_buffers_from_snapshots();
            }
        } else {
            // Recalculate this pane's effective size without the web constraint
            self.apply_effective_pane_size(&actual_pane);
        }
    }

    async fn handle_pane_input(&mut self, pane: &str, data: &[u8]) {
        // Resolve "supervisor" to actual pane name
        let actual_pane = if pane == "supervisor" {
            self.app.supervisor_name().to_string()
        } else {
            pane.to_string()
        };
        let _ = self
            .app
            .mux
            .deliver_user_input_to(&actual_pane, data, cas_mux::UserInputKind::KeyStream)
            .await;
    }

    /// Forward per-pane PTY output to web watchers.
    ///
    /// Called from handle_mux_event when PaneOutput arrives.
    pub(super) fn forward_pane_output(&self, pane_id: &str, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        // Check if anyone is watching this pane (by actual name or "supervisor" alias)
        let is_watched = self.pane_watchers.contains_key(pane_id)
            || (pane_id == self.app.supervisor_name()
                && self.pane_watchers.contains_key("supervisor"));

        if is_watched {
            if let Some(ref handle) = self.cloud_handle {
                // Send with the logical name the frontend knows
                let pane_name = if pane_id == self.app.supervisor_name() {
                    "supervisor"
                } else {
                    pane_id
                };
                handle.send_pane_output(pane_name, data.to_vec());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_plain_text_unchanged() {
        assert_eq!(strip_ansi(b"hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_removes_sgr() {
        // ESC[1;31m = bold red, ESC[0m = reset
        assert_eq!(strip_ansi(b"\x1b[1;31mERROR\x1b[0m: bad"), "ERROR: bad");
    }

    #[test]
    fn strip_ansi_removes_csi_cursor() {
        // ESC[H = cursor home, ESC[2J = clear screen
        assert_eq!(strip_ansi(b"\x1b[H\x1b[2Jhello"), "hello");
    }

    #[test]
    fn strip_ansi_removes_osc_bel() {
        // OSC terminated by BEL: ESC]0;title BEL
        let mut input = Vec::new();
        input.extend_from_slice(b"\x1b]0;my title\x07");
        input.extend_from_slice(b"content");
        assert_eq!(strip_ansi(&input), "content");
    }

    #[test]
    fn strip_ansi_removes_osc_st() {
        // OSC terminated by ST (ESC \): ESC]0;title ESC\
        assert_eq!(strip_ansi(b"\x1b]0;title\x1b\\content"), "content");
    }

    #[test]
    fn strip_ansi_removes_two_byte_escapes() {
        // ESC M = reverse index, ESC 7 = save cursor
        assert_eq!(strip_ansi(b"\x1bM\x1b7hello"), "hello");
    }

    #[test]
    fn strip_ansi_removes_carriage_return() {
        assert_eq!(strip_ansi(b"hello\r\nworld"), "hello\nworld");
    }

    #[test]
    fn strip_ansi_complex_pty_output() {
        // Simulates real PTY output with mixed sequences
        let input = b"\x1b[0m\x1b[2J\x1b[H\x1b[1;32m$\x1b[0m cargo build\r\n   \x1b[1;33mCompiling\x1b[0m cas v0.1.0\r\n";
        let result = strip_ansi(input);
        assert_eq!(result, "$ cargo build\n   Compiling cas v0.1.0\n");
    }

    #[test]
    fn pane_buffer_as_plain_text() {
        let mut buf = PaneBuffer::default();
        buf.append(b"\x1b[1;31mhello\x1b[0m world");
        assert_eq!(buf.as_plain_text(), "hello world");
    }

    #[test]
    fn snapshot_replay_reenters_alternate_screen_before_ink_redraws() {
        let snapshot = TerminalSnapshot {
            cells: vec![cas_factory_protocol::TerminalCell {
                codepoint: 'x' as u32,
                fg: (0, 0, 0),
                bg: (0, 0, 0),
                flags: 0,
                width: 1,
            }],
            cursor: cas_factory_protocol::CursorPosition { x: 0, y: 0 },
            cols: 1,
            rows: 1,
        };

        let replay = snapshot_to_ansi(&snapshot, true);
        assert!(replay.starts_with(b"\x1b[?1049h\x1b[0m\x1b[2J\x1b[H"));
        assert!(replay.ends_with(b"\x1b[0m\x1b[1;1H"));
    }
}
