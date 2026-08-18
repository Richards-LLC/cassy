use crate::ui::factory::daemon::imports::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

impl FactoryDaemon {
    pub(super) fn accept_clients(&mut self) -> anyhow::Result<bool> {
        let mut any_new = false;
        loop {
            match self.listener.accept() {
                Ok((mut stream, _addr)) => {
                    // Setup stream - skip client if any setup fails (don't crash daemon)
                    // Use non-blocking reads to prevent any single client from blocking the main loop.
                    // Write timeout prevents slow clients from stalling output.
                    if stream.set_nonblocking(true).is_err()
                        || stream
                            .set_write_timeout(Some(Duration::from_millis(50)))
                            .is_err()
                    {
                        continue;
                    }

                    let client_id = self.next_client_id;
                    self.next_client_id += 1;

                    // Send initial screen setup to new client (before inserting)
                    // Temporarily switch to blocking for the init sequence write
                    let init = b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H";
                    let _ = stream.set_nonblocking(false);
                    let write_result = stream.write_all(init);
                    let _ = stream.set_nonblocking(true);
                    if write_result.is_err() {
                        // Failed to send init, skip this client
                        continue;
                    }

                    self.clients.insert(
                        client_id,
                        ClientConnection {
                            stream,
                            input_buf: Vec::with_capacity(1024),
                            output_buf: VecDeque::with_capacity(8192),
                            needs_full_redraw: true,
                            view_mode: ClientViewMode::Full,
                            client_cols: self.cols,
                            client_rows: self.rows,
                        },
                    );
                    any_new = true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(_) => {
                    break;
                }
            }
        }
        Ok(any_new)
    }

    /// Process input from all connected clients
    pub(super) async fn process_client_input(&mut self) -> anyhow::Result<bool> {
        let client_ids: Vec<usize> = self.clients.keys().copied().collect();
        let mut disconnected = Vec::new();
        let mut all_input: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut resize_event: Option<(u16, u16)> = None;
        let mut had_activity = false;
        let now = Instant::now();

        // Read from all clients
        for client_id in client_ids {
            if let Some(client) = self.clients.get_mut(&client_id) {
                let mut buf = [0u8; 4096];
                loop {
                    match client.stream.read(&mut buf) {
                        Ok(0) => {
                            disconnected.push(client_id);
                            break;
                        }
                        Ok(n) => {
                            client.input_buf.extend_from_slice(&buf[..n]);
                        }
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut
                                || e.kind() == std::io::ErrorKind::Interrupted =>
                        {
                            // No more data available right now
                            break;
                        }
                        Err(_) => {
                            disconnected.push(client_id);
                            break;
                        }
                    }
                }

                // Parse input buffer for control sequences
                let (input_data, events) = Self::parse_client_input(&mut client.input_buf);
                let has_activity = !input_data.is_empty() || !events.is_empty();
                if has_activity {
                    let can_take = self.owner_client_id.is_none()
                        || self.owner_client_id == Some(client_id)
                        || self.owner_last_activity.elapsed()
                            > Duration::from_secs(INPUT_OWNER_IDLE_SECS);
                    if can_take {
                        self.owner_client_id = Some(client_id);
                        self.owner_last_activity = now;
                        had_activity = true;
                        if !input_data.is_empty() {
                            all_input.push((client_id, input_data));
                        }
                        for event in events {
                            match event {
                                ControlEvent::Resize(cols, rows) => {
                                    // Store per-client dimensions
                                    if let Some(c) = self.clients.get_mut(&client_id) {
                                        c.client_cols = cols;
                                        c.client_rows = rows;
                                        // Auto-detect compact mode on first resize
                                        if cols < COMPACT_WIDTH_THRESHOLD
                                            && c.view_mode == ClientViewMode::Full
                                        {
                                            c.view_mode = ClientViewMode::Compact;
                                            tracing::info!(
                                                "Client {} auto-switched to compact mode ({}x{})",
                                                client_id,
                                                cols,
                                                rows
                                            );
                                        } else if cols >= COMPACT_WIDTH_THRESHOLD
                                            && c.view_mode == ClientViewMode::Compact
                                        {
                                            c.view_mode = ClientViewMode::Full;
                                            tracing::info!(
                                                "Client {} auto-switched to full mode ({}x{})",
                                                client_id,
                                                cols,
                                                rows
                                            );
                                        }
                                        c.needs_full_redraw = true;
                                    }
                                    resize_event = Some((cols, rows));
                                }
                                ControlEvent::SetMode(mode) => {
                                    if let Some(c) = self.clients.get_mut(&client_id) {
                                        c.view_mode = mode;
                                        c.needs_full_redraw = true;
                                        tracing::info!(
                                            "Client {} set mode to {:?}",
                                            client_id,
                                            mode
                                        );
                                    }
                                }
                                ControlEvent::MouseScrollUp => {
                                    if self.app.show_changes_dialog {
                                        self.app.diff_scroll_up();
                                    } else if self.app.handle_scroll_up() == ScrollAction::AltScreen
                                    {
                                        // Focused pane is in alt-screen — forward a
                                        // harness-aware scroll payload so the inner
                                        // TUI can scroll its own transcript.
                                        // Claude/Codex: PgUp (cas-f93a).
                                        // Grok: SGR mouse wheel (cas-d3b5) — PgUp is a
                                        // no-op while the prompt is focused.
                                        let payload = self.app.alt_screen_scroll_payload(true);
                                        tracing::debug!(
                                            harness = ?self.app.focused_harness(),
                                            bytes = payload.len(),
                                            "alt-screen scroll up: forwarding wheel payload to PTY"
                                        );
                                        // Scroll is not a prompt submit.
                                        let _ = self.app.mux.send_input(&payload).await;
                                    }
                                    // ScrollAction::Done: scroll was handled by handle_scroll_up.
                                }
                                ControlEvent::MouseScrollDown => {
                                    if self.app.show_changes_dialog {
                                        self.app.diff_scroll_down();
                                    } else if self.app.handle_scroll_down()
                                        == ScrollAction::AltScreen
                                    {
                                        let payload = self.app.alt_screen_scroll_payload(false);
                                        tracing::debug!(
                                            harness = ?self.app.focused_harness(),
                                            bytes = payload.len(),
                                            "alt-screen scroll down: forwarding wheel payload to PTY"
                                        );
                                        let _ = self.app.mux.send_input(&payload).await;
                                    }
                                    // ScrollAction::Done: scroll was handled by handle_scroll_down.
                                }
                                ControlEvent::MouseClick { col, row } => {
                                    use crate::ui::factory::app::{
                                        ClickAction, ClientGeometryMode, sgr_left_click_bytes,
                                    };
                                    // Use this client's view mode so full/compact
                                    // geometry maps stay independent (cas-7f6f).
                                    let geometry = match self
                                        .clients
                                        .get(&client_id)
                                        .map(|c| c.view_mode)
                                    {
                                        Some(ClientViewMode::Compact) => {
                                            ClientGeometryMode::Compact
                                        }
                                        _ => ClientGeometryMode::Full,
                                    };
                                    match self.app.handle_mouse_click(col, row, geometry) {
                                        ClickAction::Handled => {}
                                        ClickAction::ForwardSgr {
                                            pane,
                                            col: pty_col,
                                            row: pty_row,
                                        } => {
                                            // cas-7f6f: forward SGR so Grok's on-screen
                                            // Stop can cancel inside the harness. Do NOT
                                            // clear turn_in_flight here — generic clicks
                                            // (non-Stop) must preserve active; Stop
                                            // completion comes from Grok turn_ended.
                                            let payload = sgr_left_click_bytes(pty_col, pty_row);
                                            tracing::debug!(
                                                %pane,
                                                pty_col,
                                                pty_row,
                                                ?geometry,
                                                "forwarding SGR click to focused Grok pane"
                                            );
                                            let _ =
                                                self.app.mux.send_input_to(&pane, &payload).await;
                                        }
                                    }
                                }
                                ControlEvent::SetSelectMode(on) => {
                                    self.app.select_mode = on;
                                }
                                ControlEvent::DropImage { col, row, path } => {
                                    // Mode-aware drop target (cas-7f6f): compact
                                    // clients must not use full-layout hit tests.
                                    let geometry = match self
                                        .clients
                                        .get(&client_id)
                                        .map(|c| c.view_mode)
                                    {
                                        Some(ClientViewMode::Compact) => {
                                            crate::ui::factory::app::ClientGeometryMode::Compact
                                        }
                                        _ => crate::ui::factory::app::ClientGeometryMode::Full,
                                    };
                                    let target = self.resolve_drop_target(col, row, geometry);
                                    if let Some(target_pane) = target {
                                        let _ = self.app.mux.focus(&target_pane);
                                        // StructuredPaste — never marks turn-in-flight.
                                        let payload = bracketed_paste_bytes(&path);
                                        if let Err(e) = self
                                            .app
                                            .mux
                                            .deliver_user_input_to(
                                                &target_pane,
                                                &payload,
                                                cas_mux::UserInputKind::StructuredPaste,
                                            )
                                            .await
                                        {
                                            tracing::warn!(
                                                "Failed to deliver dropped image payload to {}: {}",
                                                target_pane,
                                                e
                                            );
                                        }
                                        // Clear sidecar focus so keystrokes route
                                        // directly to the PTY pane.
                                        self.app.sidecar_focus = crate::ui::factory::director::SidecarFocus::None;
                                    } else {
                                        tracing::debug!(
                                            "Ignoring image drop outside worker/supervisor panes at ({}, {})",
                                            col,
                                            row
                                        );
                                    }
                                }
                                ControlEvent::Paste { payload } => {
                                    // Re-wrap the literal payload as a bracketed
                                    // paste and inject it into the focused pane in
                                    // one shot. Delivering it as a single paste
                                    // (not the raw byte stream that the input parser
                                    // walks one byte at a time) keeps embedded
                                    // newlines/control bytes literal, so the inner
                                    // CLI stops submitting mid-paste (cas-5702).
                                    // StructuredPaste — never marks turn-in-flight.
                                    if self.app.focused_accepts_input() {
                                        let bytes = bracketed_paste_bytes(&payload);
                                        if let Err(e) = self
                                            .app
                                            .mux
                                            .deliver_user_input(
                                                &bytes,
                                                cas_mux::UserInputKind::StructuredPaste,
                                            )
                                            .await
                                        {
                                            tracing::warn!(
                                                "Failed to deliver paste payload: {}",
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Remove disconnected clients and recalibrate if any left
        let had_disconnects = !disconnected.is_empty();
        for id in disconnected {
            self.clients.remove(&id);
            if self.owner_client_id == Some(id) {
                self.owner_client_id = None;
            }
        }
        if had_disconnects {
            self.recalibrate_after_disconnect();
        }

        // Debounce resize: store pending event, apply in main loop after 100ms
        if let Some((cols, rows)) = resize_event {
            self.pending_resize = Some((cols, rows));
            self.pending_resize_at = Instant::now();
        }

        // Process keyboard input (use first client's input)
        for (_client_id, input) in all_input {
            self.process_input(&input).await?;
        }

        Ok(had_activity)
    }

    /// Parse client input, extracting control sequences
    fn parse_client_input(buf: &mut Vec<u8>) -> (Vec<u8>, Vec<ControlEvent>) {
        let mut input_data = Vec::new();
        let mut events = Vec::new();
        let mut i = 0;

        while i < buf.len() {
            // Check for control sequence prefix
            if buf[i..].starts_with(CONTROL_PREFIX) {
                // Find the end of the control sequence (BEL)
                if let Some(end_offset) = buf[i + CONTROL_PREFIX.len()..]
                    .iter()
                    .position(|&b| b == CONTROL_SUFFIX)
                {
                    let cmd_start = i + CONTROL_PREFIX.len();
                    let cmd_end = cmd_start + end_offset;
                    let cmd = &buf[cmd_start..cmd_end];

                    // Parse command
                    if let Ok(cmd_str) = std::str::from_utf8(cmd) {
                        if cmd_str.starts_with("resize;") {
                            let parts: Vec<&str> = cmd_str.split(';').collect();
                            if parts.len() == 3 {
                                if let (Ok(cols), Ok(rows)) =
                                    (parts[1].parse::<u16>(), parts[2].parse::<u16>())
                                {
                                    events.push(ControlEvent::Resize(cols, rows));
                                }
                            }
                        } else if cmd_str.starts_with("mouse;") {
                            // Format: mouse;kind;col;row
                            let parts: Vec<&str> = cmd_str.split(';').collect();
                            if parts.len() == 4 {
                                match parts[1] {
                                    "scroll_up" => events.push(ControlEvent::MouseScrollUp),
                                    "scroll_down" => events.push(ControlEvent::MouseScrollDown),
                                    "click" => {
                                        if let (Ok(col), Ok(row)) =
                                            (parts[2].parse::<u16>(), parts[3].parse::<u16>())
                                        {
                                            events.push(ControlEvent::MouseClick { col, row });
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        } else if let Some(rest) = cmd_str.strip_prefix("drop_image;") {
                            // Format: drop_image;col;row;base64_payload
                            let parts: Vec<&str> = rest.splitn(3, ';').collect();
                            if parts.len() == 3 {
                                if let (Ok(col), Ok(row)) =
                                    (parts[0].parse::<u16>(), parts[1].parse::<u16>())
                                {
                                    if let Ok(decoded) = URL_SAFE_NO_PAD.decode(parts[2]) {
                                        if let Ok(payload) = String::from_utf8(decoded) {
                                            events.push(ControlEvent::DropImage {
                                                col,
                                                row,
                                                path: payload,
                                            });
                                        }
                                    }
                                }
                            }
                        } else if let Some(rest) = cmd_str.strip_prefix("paste;") {
                            // Format: paste;base64_payload — the literal pasted
                            // text, base64-encoded so newlines / ';' / the BEL
                            // control suffix can't corrupt the framing (cas-5702).
                            if let Ok(decoded) = URL_SAFE_NO_PAD.decode(rest) {
                                if let Ok(payload) = String::from_utf8(decoded) {
                                    events.push(ControlEvent::Paste { payload });
                                }
                            }
                        } else if let Some(mode_str) = cmd_str.strip_prefix("mode;") {
                            match mode_str {
                                "compact" => {
                                    events.push(ControlEvent::SetMode(ClientViewMode::Compact))
                                }
                                "full" => events.push(ControlEvent::SetMode(ClientViewMode::Full)),
                                "select_on" => events.push(ControlEvent::SetSelectMode(true)),
                                "select_off" => events.push(ControlEvent::SetSelectMode(false)),
                                _ => {}
                            }
                        } else if cmd_str == "detach" {
                            // Client wants to detach - just disconnect them
                        }
                    }

                    i = cmd_end + 1; // Skip past BEL
                    continue;
                }
            }

            // Regular input byte
            input_data.push(buf[i]);
            i += 1;
        }

        buf.clear();
        (input_data, events)
    }

    /// Process input bytes (handles escape sequences for arrow keys)
    async fn process_input(&mut self, input: &[u8]) -> anyhow::Result<()> {
        use crate::ui::factory::input::LayoutSizes;

        let mut i = 0;
        while i < input.len() {
            let byte = input[i];

            // Modal dialog interception - when a dialog is open, capture all input
            if self.app.show_task_dialog || self.app.show_changes_dialog || self.app.show_help {
                // Check for escape sequence (arrow keys: ESC [ A/B)
                if byte == 0x1b && i + 2 < input.len() && input[i + 1] == b'[' {
                    if self.app.show_changes_dialog {
                        match input[i + 2] {
                            b'A' => self.app.diff_scroll_up(),   // Up arrow
                            b'B' => self.app.diff_scroll_down(), // Down arrow
                            b'5' if i + 3 < input.len() && input[i + 3] == b'~' => {
                                self.app.diff_page_up(); // PageUp
                                i += 4;
                                continue;
                            }
                            b'6' if i + 3 < input.len() && input[i + 3] == b'~' => {
                                self.app.diff_page_down(); // PageDown
                                i += 4;
                                continue;
                            }
                            _ => {}
                        }
                    } else {
                        match input[i + 2] {
                            // Arrow keys in task-dialog / help overlay:
                            // handle_scroll_up/down will route to the dialog's scroll
                            // or host scrollback; AltScreen is suppressed in those
                            // contexts so we discard the return value.
                            b'A' => { self.app.handle_scroll_up(); }   // Up arrow
                            b'B' => { self.app.handle_scroll_down(); } // Down arrow
                            _ => {}
                        }
                    }
                    i += 3;
                    continue;
                }

                // Standalone ESC = cancel search mode or close dialog
                if byte == 0x1b {
                    if self.app.is_diff_search_mode() {
                        self.app.cancel_diff_search();
                    } else if self.app.show_help {
                        self.app.show_help = false;
                    } else {
                        self.app.handle_escape();
                    }
                    i += 1;
                    continue;
                }

                // Changes dialog has rich key handling
                if self.app.show_changes_dialog {
                    if self.app.is_diff_search_mode() {
                        match byte {
                            0x0d => self.app.confirm_diff_search(),          // Enter
                            0x7f => self.app.handle_diff_search_backspace(), // Backspace
                            0x1b => self.app.cancel_diff_search(), // Esc (standalone, already checked above)
                            c if (0x20..0x7f).contains(&c) => {
                                self.app.handle_diff_search_char(c as char)
                            }
                            _ => {}
                        }
                    } else {
                        match byte {
                            b'j' => self.app.diff_scroll_down(),
                            b'k' => self.app.diff_scroll_up(),
                            b']' => self.app.diff_next_hunk(),
                            b'[' => self.app.diff_prev_hunk(),
                            b's' => self.app.diff_toggle_style(),
                            b'/' => self.app.start_diff_search(),
                            b'n' => self.app.next_diff_match(),
                            b'N' => self.app.prev_diff_match(),
                            b'd' => self.app.diff_cycle_inline_mode(),
                            b'l' => self.app.diff_toggle_line_numbers(),
                            b'e' => self.app.diff_toggle_expand_all(),
                            _ => {}
                        }
                    }
                } else {
                    match byte {
                        // Vim-style scroll bindings.  AltScreen is not forwarded
                        // here (these bindings predate the alt-screen path); the
                        // return value is intentionally discarded.
                        b'j' => { self.app.handle_scroll_down(); }
                        b'k' => { self.app.handle_scroll_up(); }
                        b'?' => {
                            if self.app.show_help {
                                self.app.show_help = false;
                            }
                        }
                        _ => {} // Consume all other keys
                    }
                }
                i += 1;
                continue;
            }

            // Terminal dialog interception - forward all input to shell PTY
            if self.app.show_terminal_dialog {
                // Ctrl+T (0x14) = hide terminal (shell keeps running)
                if byte == 0x14 {
                    self.app.hide_terminal_dialog();
                    i += 1;
                    continue;
                }
                // Ctrl+D (0x04) = kill terminal shell and close dialog
                if byte == 0x04 {
                    self.app.kill_terminal();
                    i += 1;
                    continue;
                }
                // Forward everything else to the shell PTY (preserving escape sequences)
                if let Some(ref name) = self.app.terminal_pane_name {
                    let name = name.clone();
                    let chunk_len = if byte == 0x1b && i + 2 < input.len() && input[i + 1] == b'[' {
                        // CSI sequence: ESC [ ... final_byte
                        let mut end = i + 2;
                        while end < input.len() && input[end] < 0x40 {
                            end += 1;
                        }
                        if end < input.len() {
                            end + 1 - i
                        } else {
                            input.len() - i
                        }
                    } else {
                        1
                    };
                    if let Some(pane) = self.app.mux.get(&name) {
                        let _ = pane.write(&input[i..i + chunk_len]).await;
                    }
                    i += chunk_len;
                } else {
                    i += 1;
                }
                continue;
            }

            // Handle resize mode first - intercepts all input
            if self.app.input_mode.is_resize() {
                // Arrow keys (ESC [ A/B/C/D)
                if byte == 0x1b && i + 2 < input.len() && input[i + 1] == b'[' {
                    let arrow = input[i + 2];
                    match arrow {
                        b'A' => self
                            .app
                            .resize_layout(|s| s.grow_supervisor(LayoutSizes::STEP)),
                        b'B' => self
                            .app
                            .resize_layout(|s| s.shrink_supervisor(LayoutSizes::STEP)),
                        b'C' => self
                            .app
                            .resize_layout(|s| s.grow_workers(LayoutSizes::STEP)),
                        b'D' => self
                            .app
                            .resize_layout(|s| s.shrink_workers(LayoutSizes::STEP)),
                        _ => {}
                    }
                    i += 3;
                    continue;
                }
                match byte {
                    b'h' => self
                        .app
                        .resize_layout(|s| s.shrink_workers(LayoutSizes::STEP)),
                    b'H' => self
                        .app
                        .resize_layout(|s| s.shrink_workers(LayoutSizes::LARGE_STEP)),
                    b'l' => self
                        .app
                        .resize_layout(|s| s.grow_workers(LayoutSizes::STEP)),
                    b'L' => self
                        .app
                        .resize_layout(|s| s.grow_workers(LayoutSizes::LARGE_STEP)),
                    b'j' => self
                        .app
                        .resize_layout(|s| s.shrink_supervisor(LayoutSizes::STEP)),
                    b'J' => self
                        .app
                        .resize_layout(|s| s.shrink_supervisor(LayoutSizes::LARGE_STEP)),
                    b'k' => self
                        .app
                        .resize_layout(|s| s.grow_supervisor(LayoutSizes::STEP)),
                    b'K' => self
                        .app
                        .resize_layout(|s| s.grow_supervisor(LayoutSizes::LARGE_STEP)),
                    b'r' => self.app.reset_layout(),
                    0x1b | b'\r' => self.app.exit_resize_mode(),
                    b'q' | b'Q' => self.shutdown.store(true, Ordering::Relaxed),
                    _ => {}
                }
                i += 1;
                continue;
            }

            // Mission Control mode input handling
            if self.app.is_mission_control() {
                // Ctrl+W toggles back to Panes view
                if byte == 0x17 {
                    self.app.toggle_factory_view_mode();
                    i += 1;
                    continue;
                }
                // Arrow keys in MC mode
                if byte == 0x1b && i + 2 < input.len() && input[i + 1] == b'[' {
                    match input[i + 2] {
                        b'A' => self.app.mc_scroll_up(),   // Up
                        b'B' => self.app.mc_scroll_down(), // Down
                        _ => {}
                    }
                    i += 3;
                    continue;
                }
                // Standalone ESC in MC mode
                if byte == 0x1b {
                    let is_standalone_esc = i + 1 >= input.len() || input[i + 1] != b'[';
                    if is_standalone_esc {
                        self.app.mc_handle_escape();
                        i += 1;
                        continue;
                    }
                }
                // Regular keys in MC mode
                match byte {
                    b'\t' => self.app.mc_focus_next(),
                    b'j' | b'J' => self.app.mc_scroll_down(),
                    b'k' | b'K' => self.app.mc_scroll_up(),
                    b'\r' => self.app.mc_handle_enter(),
                    b' ' => self.app.mc_toggle_collapse(),
                    b'i' | b'I' => self.app.mc_start_inject(),
                    // Direct panel jump keys
                    b'w' | b'W' => self
                        .app
                        .mc_focus_panel(crate::ui::factory::renderer::MissionControlFocus::Workers),
                    b't' | b'T' => self
                        .app
                        .mc_focus_panel(crate::ui::factory::renderer::MissionControlFocus::Tasks),
                    b'c' | b'C' => self
                        .app
                        .mc_focus_panel(crate::ui::factory::renderer::MissionControlFocus::Changes),
                    b'a' | b'A' => self.app.mc_focus_panel(
                        crate::ui::factory::renderer::MissionControlFocus::Activity,
                    ),
                    b'?' => self.app.show_help = !self.app.show_help,
                    b'q' | b'Q' => self.shutdown.store(true, Ordering::Relaxed),
                    _ => {}
                }
                i += 1;
                continue;
            }

            // Check for Ctrl+Arrow sequences (ESC [ 1 ; 5 C/D) — pane cycling
            // Must be checked before basic arrow handling since both start with ESC [
            if byte == 0x1b
                && i + 5 < input.len()
                && input[i + 1] == b'['
                && input[i + 2] == b'1'
                && input[i + 3] == b';'
                && input[i + 4] == b'5'
                && matches!(input[i + 5], b'C' | b'D')
            {
                match input[i + 5] {
                    b'C' => self.app.focus_next_pty_pane(), // Ctrl+Right
                    b'D' => self.app.focus_prev_pty_pane(), // Ctrl+Left
                    _ => {}
                }
                i += 6;
                continue;
            }

            // PgUp (ESC [ 5 ~) / PgDn (ESC [ 6 ~) — must be checked before the
            // generic ESC-sequence handler that only reads 3 bytes.
            if byte == 0x1b
                && i + 3 < input.len()
                && input[i + 1] == b'['
                && matches!(input[i + 2], b'5' | b'6')
                && input[i + 3] == b'~'
            {
                let is_pgup = input[i + 2] == b'5';
                if self.app.show_changes_dialog {
                    if is_pgup {
                        self.app.diff_scroll_up();
                    } else {
                        self.app.diff_scroll_down();
                    }
                } else {
                    // Route through the unified dispatch helper.  When the focused pane
                    // is in alt-screen the helper returns AltScreen — send the native
                    // page-scroll sequence so the inner TUI can handle paging (PgUp/PgDn
                    // is more than 3 lines; let the app decide).  Otherwise Done means
                    // the scroll was handled internally (dialog, sidecar, MC, scrollback).
                    let action = if is_pgup {
                        self.app.handle_scroll_up()
                    } else {
                        self.app.handle_scroll_down()
                    };
                    if action == ScrollAction::AltScreen {
                        let seq: &[u8] = if is_pgup { b"\x1b[5~" } else { b"\x1b[6~" };
                        tracing::debug!(
                            "alt-screen pg{}: forwarding {:?} to PTY",
                            if is_pgup { "up" } else { "dn" },
                            seq,
                        );
                        let _ = self.app.mux.send_input(seq).await;
                    }
                }
                i += 4;
                continue;
            }

            // Check for escape sequence (arrow keys: ESC [ A/B/C/D)
            if byte == 0x1b && i + 2 < input.len() && input[i + 1] == b'[' {
                let arrow = input[i + 2];
                let is_arrow = matches!(arrow, b'A' | b'B' | b'C' | b'D');

                if is_arrow {
                    if self.app.sidecar_is_focused() {
                        // Handle arrows in sidecar
                        match arrow {
                            b'A' => self.app.sidecar_scroll_up(),   // Up
                            b'B' => self.app.sidecar_scroll_down(), // Down
                            _ => {}                                 // Left/Right ignored in sidecar
                        }
                        i += 3;
                        continue;
                    } else if self.app.focused_accepts_input() {
                        // Forward arrow keys to focused pane
                        let _ = self.app.mux.send_input(&input[i..i + 3]).await;
                        i += 3;
                        continue;
                    }
                }
            }

            // Check for standalone ESC (not followed by '[' or at end of input)
            // Give it a chance to be part of a sequence by checking next byte
            if byte == 0x1b {
                let is_standalone_esc = i + 1 >= input.len() || input[i + 1] != b'[';
                if is_standalone_esc && self.app.sidecar_is_focused() {
                    // Use handle_escape which properly handles view mode transitions
                    self.app.handle_escape();
                    i += 1;
                    continue;
                } else if is_standalone_esc && self.app.focused_accepts_input() {
                    // cas-7f6f: Grok mid-turn Esc is a no-op (0.2.93). Only
                    // rewrite Esc→cancel when turn_in_flight is set (submit/
                    // inject; cleared by break_turn or Grok turn_ended).
                    // Idle keeps raw Esc for clear/rewind.
                    use crate::ui::factory::app::GrokEscAction;
                    if self.app.focused_harness() == cas_mux::SupervisorCli::Grok {
                        match self.app.focused_grok_esc_action() {
                            GrokEscAction::CancelTurn => {
                                if let Some(id) = self.app.mux.focused_id().map(|s| s.to_string())
                                {
                                    tracing::debug!(
                                        pane = %id,
                                        "Grok Esc (turn active) → cancel via break_turn"
                                    );
                                    let _ = self.app.mux.break_turn(&id).await;
                                }
                            }
                            GrokEscAction::ForwardRaw => {
                                tracing::debug!("Grok Esc (idle) → raw Esc");
                                let _ = self.app.mux.send_input(&[byte]).await;
                            }
                        }
                    } else {
                        // Claude/Codex: Esc is the native cancel key — forward raw.
                        let _ = self.app.mux.send_input(&[byte]).await;
                    }
                    i += 1;
                    continue;
                } else if !is_standalone_esc {
                    // Part of unknown escape sequence - forward to focused pane if applicable
                    if self.app.focused_accepts_input() {
                        let _ = self.app.mux.send_input(&[byte]).await;
                    }
                    i += 1;
                    continue;
                }
            }

            // Ctrl+D (0x04) = detach - client handles this, just ignore
            if byte == 0x04 {
                i += 1;
                continue;
            }

            // Ctrl+E (0x05) = dismiss active error banner
            if byte == 0x05 {
                self.app.clear_error();
                i += 1;
                continue;
            }

            // Ctrl+Q (0x11) = quit daemon
            if byte == 0x11 {
                self.shutdown.store(true, Ordering::Relaxed);
                i += 1;
                continue;
            }

            // Ctrl+R (0x12) = refresh Cassy data (global)
            if byte == 0x12 {
                let _ = self.app.refresh_data();
                i += 1;
                continue;
            }

            // Ctrl+C (0x03) = interrupt focused pane (debounced)
            if byte == 0x03 {
                let now = Instant::now();
                if self
                    .app
                    .last_interrupt_time
                    .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(2))
                {
                    self.app.last_interrupt_time = Some(now);
                    let _ = self.app.mux.interrupt_focused().await;
                }
                i += 1;
                continue;
            }

            // Ctrl+T (0x14) = open/hide terminal dialog
            if byte == 0x14 {
                if self.app.show_terminal_dialog {
                    self.app.hide_terminal_dialog();
                } else {
                    self.app.open_terminal_dialog();
                }
                i += 1;
                continue;
            }

            // Ctrl+N (0x0E) = enter resize mode (global)
            if byte == 0x0E {
                self.app.enter_resize_mode();
                i += 1;
                continue;
            }

            // Ctrl+] (0x1D) = collapse/expand entire sidebar (global)
            if byte == 0x1d {
                self.app.toggle_sidecar_collapsed();
                i += 1;
                continue;
            }

            // Ctrl+W (0x17) = toggle factory view mode (Panes ↔ Mission Control)
            if byte == 0x17 {
                self.app.toggle_factory_view_mode();
                i += 1;
                continue;
            }

            // Check if sidecar is focused first - handle sidecar navigation
            if self.app.sidecar_is_focused() {
                match byte {
                    // Tab or Ctrl+P = cycle sidecar panels, exit to supervisor on last panel
                    b'\t' | 0x10 => {
                        if self.app.sidecar_focus
                            == crate::ui::factory::director::SidecarFocus::Activity
                        {
                            // Last panel, exit to supervisor
                            self.app.toggle_sidecar_focus();
                            let sup_name = self.app.supervisor_name().to_string();
                            let _ = self.app.mux.focus(&sup_name);
                        } else {
                            self.app.next_sidecar_panel();
                        }
                    }
                    // j = scroll down
                    b'j' | b'J' => {
                        self.app.sidecar_scroll_down();
                    }
                    // k = scroll up
                    b'k' | b'K' => {
                        self.app.sidecar_scroll_up();
                    }
                    // Enter = open detail view
                    b'\r' => {
                        self.app.handle_enter();
                    }
                    // Space = toggle epic/directory collapse
                    b' ' => {
                        self.app.toggle_collapse();
                    }
                    // 'q' = quit
                    b'q' | b'Q' => {
                        self.shutdown.store(true, Ordering::Relaxed);
                    }
                    _ => {}
                }
                i += 1;
                continue;
            }

            // If focused pane accepts input, forward most input
            if self.app.focused_accepts_input() {
                // Ctrl+P (0x10) = toggle sidecar focus (use Ctrl+P instead of Tab
                // so Tab flows through to the PTY for autocomplete acceptance)
                if byte == 0x10 {
                    self.app.toggle_sidecar_focus();
                    i += 1;
                    continue;
                }

                // Unified submit API (cas-7f6f): KeyStream marks on true Enter;
                // paste/drop use StructuredPaste elsewhere and never mark here.
                let _ = self
                    .app
                    .mux
                    .deliver_user_input(&[byte], cas_mux::UserInputKind::KeyStream)
                    .await;
                i += 1;
                continue;
            }

            // When focused on a non-input pane, handle navigation keys
            match byte {
                // Tab or Ctrl+P = go to sidecar (workers are view-only, not in tab cycle)
                b'\t' | 0x10 => {
                    self.app.toggle_sidecar_focus();
                }
                // 'p' or 'd' = toggle sidecar panel focus
                b'p' | b'P' | b'd' | b'D' => {
                    self.app.toggle_sidecar_focus();
                }
                // 's' = focus supervisor
                b's' | b'S' => {
                    let sup_name = self.app.supervisor_name().to_string();
                    let _ = self.app.mux.focus(&sup_name);
                }
                // '1'-'9' = focus worker by number
                b'1'..=b'9' => {
                    let idx = (byte - b'0') as usize;
                    self.app.select_worker_by_number(idx);
                }
                // 'q' = quit
                b'q' | b'Q' => {
                    self.shutdown.store(true, Ordering::Relaxed);
                }
                _ => {}
            }

            i += 1;
        }

        Ok(())
    }

    fn resolve_drop_target(
        &self,
        col: u16,
        row: u16,
        geometry: crate::ui::factory::app::ClientGeometryMode,
    ) -> Option<String> {
        if col != u16::MAX && row != u16::MAX {
            if let Some(pane_name) = self.app.pane_at_screen_for(col, row, geometry) {
                if pane_name == self.app.supervisor_name()
                    || self.app.worker_names().iter().any(|w| w == &pane_name)
                {
                    return Some(pane_name);
                }
            }
        }

        // Compact clients only show the supervisor content area.
        if geometry == crate::ui::factory::app::ClientGeometryMode::Compact {
            return Some(self.app.supervisor_name().to_string());
        }

        if self.app.focused_is_supervisor() {
            return Some(self.app.supervisor_name().to_string());
        }

        self.app.mux.focused().and_then(|pane| match pane.kind() {
            cas_mux::PaneKind::Worker | cas_mux::PaneKind::Supervisor => {
                Some(pane.id().to_string())
            }
            _ => None,
        })
    }
}

fn bracketed_paste_bytes(payload: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(payload.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}
