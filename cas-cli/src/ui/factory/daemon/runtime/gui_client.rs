use crate::ui::factory::daemon::imports::*;
use crate::ui::factory::protocol::{
    ClientMessage, DaemonMessage, FRAME_HEADER_SIZE, MAX_MESSAGE_SIZE, PaneInfo, PaneKind,
    SessionState, decode_length, encode_message,
};

/// Encode a DaemonMessage into a framed byte vector, or None on error.
fn encode_frame(msg: &DaemonMessage) -> Option<Vec<u8>> {
    encode_message(msg).ok()
}

/// Queue a pre-encoded frame into a GUI client's write buffer.
fn queue_frame(client: &mut GuiConnection, frame: &[u8]) {
    client.write_buf.extend(frame);
}

impl FactoryDaemon {
    /// Accept new GUI client connections (non-blocking).
    pub(super) fn accept_gui_clients(&mut self) -> bool {
        let mut any_new = false;
        loop {
            match self.gui_listener.accept() {
                Ok((stream, _addr)) => {
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }

                    let client_id = self.next_gui_client_id;
                    self.next_gui_client_id += 1;

                    // Build and send Welcome message with current state
                    let state = self.build_session_state();
                    let scrollback = self.build_scrollback();
                    let welcome = DaemonMessage::Welcome {
                        session_name: self.session_name.clone(),
                        state,
                        scrollback: Some(scrollback),
                    };

                    let frame = match encode_frame(&welcome) {
                        Some(f) => f,
                        None => continue,
                    };

                    let mut client = GuiConnection {
                        stream,
                        read_buf: Vec::with_capacity(4096),
                        write_buf: VecDeque::with_capacity(8192),
                        pane_sizes: HashMap::new(),
                    };

                    // Blocking write for the initial Welcome
                    let _ = client.stream.set_nonblocking(false);
                    if client.stream.write_all(&frame).is_err() {
                        continue;
                    }
                    let _ = client.stream.set_nonblocking(true);

                    tracing::info!("GUI client {} connected", client_id);
                    self.gui_clients.insert(client_id, client);
                    any_new = true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        any_new
    }

    /// Process input from all GUI clients, returning whether any activity occurred.
    pub(super) async fn process_gui_client_input(&mut self) -> bool {
        let client_ids: Vec<usize> = self.gui_clients.keys().copied().collect();
        let mut disconnected = Vec::new();
        let mut messages: Vec<(usize, ClientMessage)> = Vec::new();

        for client_id in client_ids {
            if let Some(client) = self.gui_clients.get_mut(&client_id) {
                // Read available bytes
                let mut buf = [0u8; 4096];
                loop {
                    match client.stream.read(&mut buf) {
                        Ok(0) => {
                            disconnected.push(client_id);
                            break;
                        }
                        Ok(n) => {
                            client.read_buf.extend_from_slice(&buf[..n]);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => {
                            disconnected.push(client_id);
                            break;
                        }
                    }
                }

                // Parse complete messages from the read buffer
                loop {
                    if client.read_buf.len() < FRAME_HEADER_SIZE {
                        break;
                    }
                    let header: [u8; FRAME_HEADER_SIZE] =
                        client.read_buf[..FRAME_HEADER_SIZE].try_into().unwrap();
                    let msg_len = decode_length(&header);
                    if msg_len > MAX_MESSAGE_SIZE {
                        tracing::warn!(
                            "GUI client {} sent oversized message ({} bytes)",
                            client_id,
                            msg_len
                        );
                        disconnected.push(client_id);
                        break;
                    }
                    let total = FRAME_HEADER_SIZE + msg_len;
                    if client.read_buf.len() < total {
                        break;
                    }
                    let json_bytes = &client.read_buf[FRAME_HEADER_SIZE..total];
                    match serde_json::from_slice::<ClientMessage>(json_bytes) {
                        Ok(msg) => messages.push((client_id, msg)),
                        Err(e) => {
                            tracing::warn!("GUI client {} sent invalid message: {}", client_id, e);
                        }
                    }
                    client.read_buf.drain(..total);
                }
            }
        }

        for id in &disconnected {
            let had_sizes = self
                .gui_clients
                .get(id)
                .is_some_and(|c| !c.pane_sizes.is_empty());
            self.gui_clients.remove(id);
            tracing::info!("GUI client {} disconnected", id);
            if had_sizes {
                self.reconcile_after_gui_disconnect();
            }
        }

        let had_activity = !messages.is_empty();
        for (client_id, msg) in messages {
            self.handle_gui_message(client_id, msg).await;
        }
        had_activity
    }

    /// Flush pending output to all GUI clients.
    pub(super) fn flush_gui_client_output(&mut self) {
        let mut disconnected = Vec::new();

        for (id, client) in self.gui_clients.iter_mut() {
            if client.write_buf.is_empty() {
                continue;
            }
            let (front, back) = client.write_buf.as_slices();
            let chunk = if !front.is_empty() { front } else { back };
            if chunk.is_empty() {
                continue;
            }
            match client.stream.write(chunk) {
                Ok(0) => disconnected.push(*id),
                Ok(n) => {
                    for _ in 0..n {
                        client.write_buf.pop_front();
                    }
                }
                Err(e) => match e.kind() {
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => {}
                    _ => disconnected.push(*id),
                },
            }
        }

        for id in disconnected {
            let had_sizes = self
                .gui_clients
                .get(&id)
                .is_some_and(|c| !c.pane_sizes.is_empty());
            self.gui_clients.remove(&id);
            tracing::info!("GUI client {} disconnected (write error)", id);
            if had_sizes {
                self.reconcile_after_gui_disconnect();
            }
        }
    }

    /// Forward per-pane PTY output to all connected GUI clients.
    pub(super) fn forward_pane_output_to_gui(&mut self, pane_id: &str, data: &[u8]) {
        if self.gui_clients.is_empty() || data.is_empty() {
            return;
        }
        let msg = DaemonMessage::Output {
            pane_id: pane_id.to_string(),
            data: data.to_vec(),
        };
        if let Some(frame) = encode_frame(&msg) {
            for client in self.gui_clients.values_mut() {
                queue_frame(client, &frame);
            }
        }
    }

    /// Notify all protocol clients (GUI + WS) that a pane was added.
    pub(super) fn notify_pane_added(&mut self, pane_id: &str, kind: cas_mux::PaneKind) {
        if self.gui_clients.is_empty() && self.ws_clients.is_empty() {
            return;
        }
        let msg = DaemonMessage::PaneAdded {
            pane: PaneInfo {
                id: pane_id.to_string(),
                kind: PaneKind::from(kind),
                focused: false,
                title: pane_id.to_string(),
                exited: false,
            },
        };
        self.broadcast_daemon_message(&msg);
    }

    /// Notify all protocol clients (GUI + WS) that a pane was removed.
    pub(super) fn notify_pane_removed(&mut self, pane_id: &str) {
        if self.gui_clients.is_empty() && self.ws_clients.is_empty() {
            return;
        }
        let msg = DaemonMessage::PaneRemoved {
            pane_id: pane_id.to_string(),
        };
        self.broadcast_daemon_message(&msg);
    }

    /// Notify all protocol clients (GUI + WS) that a pane exited.
    pub(super) fn notify_pane_exited(&mut self, pane_id: &str, exit_code: Option<i32>) {
        if self.gui_clients.is_empty() && self.ws_clients.is_empty() {
            return;
        }
        let msg = DaemonMessage::PaneExited {
            pane_id: pane_id.to_string(),
            exit_code,
        };
        self.broadcast_daemon_message(&msg);
    }

    /// Send periodic state updates to all protocol clients (GUI + WS).
    pub(super) fn send_state_update(&mut self) {
        if self.gui_clients.is_empty() && self.ws_clients.is_empty() {
            return;
        }
        let state = self.build_session_state();
        let msg = DaemonMessage::StateUpdate { state };
        self.broadcast_daemon_message(&msg);
    }

    // Legacy aliases for backward compatibility with callers
    pub(super) fn gui_notify_pane_added(&mut self, pane_id: &str, kind: cas_mux::PaneKind) {
        self.notify_pane_added(pane_id, kind);
    }
    pub(super) fn gui_notify_pane_removed(&mut self, pane_id: &str) {
        self.notify_pane_removed(pane_id);
    }
    pub(super) fn gui_notify_pane_exited(&mut self, pane_id: &str, exit_code: Option<i32>) {
        self.notify_pane_exited(pane_id, exit_code);
    }
    #[allow(dead_code)]
    pub(super) fn gui_send_state_update(&mut self) {
        self.send_state_update();
    }

    /// Handle a single ClientMessage from a GUI client.
    async fn handle_gui_message(&mut self, client_id: usize, msg: ClientMessage) {
        match msg {
            ClientMessage::Attach { .. } => {
                let state = self.build_session_state();
                let scrollback = self.build_scrollback();
                let welcome = DaemonMessage::Welcome {
                    session_name: self.session_name.clone(),
                    state,
                    scrollback: Some(scrollback),
                };
                if let Some(frame) = encode_frame(&welcome) {
                    if let Some(client) = self.gui_clients.get_mut(&client_id) {
                        queue_frame(client, &frame);
                    }
                }
            }
            ClientMessage::Detach => {
                if let Some(frame) = encode_frame(&DaemonMessage::Detached) {
                    if let Some(client) = self.gui_clients.get_mut(&client_id) {
                        queue_frame(client, &frame);
                    }
                }
                let had_sizes = self
                    .gui_clients
                    .get(&client_id)
                    .is_some_and(|c| !c.pane_sizes.is_empty());
                self.gui_clients.remove(&client_id);
                tracing::info!("GUI client {} detached", client_id);
                if had_sizes {
                    self.reconcile_after_gui_disconnect();
                }
            }
            ClientMessage::Input { pane_id, data } => {
                let actual = self.resolve_pane_name(&pane_id);
                // Unified KeyStream submit API (cas-7f6f). Paste/drop are not
                // delivered on this message path.
                let _ = self
                    .app
                    .mux
                    .deliver_user_input_to(&actual, &data, cas_mux::UserInputKind::KeyStream)
                    .await;
            }
            ClientMessage::InputFocused { data } => {
                let _ = self
                    .app
                    .mux
                    .deliver_user_input(&data, cas_mux::UserInputKind::KeyStream)
                    .await;
            }
            ClientMessage::Focus { pane_id } => {
                let actual = self.resolve_pane_name(&pane_id);
                let _ = self.app.mux.focus(&actual);
            }
            ClientMessage::FocusNext => {
                self.app.mux.focus_next();
            }
            ClientMessage::FocusPrev => {
                self.app.mux.focus_prev();
            }
            ClientMessage::Resize { cols, rows } => {
                tracing::debug!(
                    "GUI client {} reported global resize: {}x{}",
                    client_id,
                    cols,
                    rows
                );
            }
            ClientMessage::ResizePane {
                pane_id,
                cols,
                rows,
            } => {
                let actual = self.resolve_pane_name(&pane_id);

                // Store this client's size for the pane
                if let Some(client) = self.gui_clients.get_mut(&client_id) {
                    client.pane_sizes.insert(actual.clone(), (cols, rows));
                }

                self.apply_effective_pane_size(&actual);
            }
            ClientMessage::SpawnWorkers {
                count,
                names,
                specs,
            } => {
                if names.is_empty() {
                    self.app.spawning_count += count;
                    for i in 0..count {
                        let spec = specs.get(i).cloned().flatten();
                        self.pending_spawns.push_back(PendingSpawn::Anonymous {
                            isolate: false,
                            spec,
                            // cas-6913: task_id pre-assignment is MCP-only for
                            // now — the GUI client protocol has no task_id field.
                            task_id: None,
                        });
                    }
                } else {
                    self.app.spawning_count += names.len();
                    for (i, name) in names.into_iter().enumerate() {
                        let spec = specs.get(i).cloned().flatten();
                        self.pending_spawns.push_back(PendingSpawn::Named {
                            name,
                            isolate: false,
                            spec,
                            task_id: None,
                        });
                    }
                }
            }
            ClientMessage::ShutdownWorkers { count, names } => {
                self.pending_spawns.push_back(PendingSpawn::Shutdown {
                    count: Some(count),
                    names,
                    force: false,
                });
            }
            ClientMessage::Inject {
                pane_id,
                prompt,
                urgent,
            } => {
                let actual = self.resolve_pane_name(&pane_id);
                if urgent {
                    // Urgent: interrupt-and-redirect by name via the PTY,
                    // bypassing the inbox even in teams mode (cas-c931).
                    // cas-ab80: frame Codex payloads with the same sender
                    // contract as normal delivery (director is the inject source).
                    let harness = self.app.harness_for(&actual);
                    let payload = super::delivery::frame_pty_payload(
                        harness,
                        super::teams::DIRECTOR_AGENT_NAME,
                        &prompt,
                    );
                    let settle = self.urgent_settle_duration(&actual);
                    let _ = self
                        .app
                        .mux
                        .interrupt_and_inject(&actual, &payload, settle)
                        .await;
                } else {
                    // Recipient-aware routing (cas-b68a): inject reaches a Codex pane
                    // via PTY even when the supervisor runs Claude teams.
                    // color=Some(DIRECTOR_AGENT_COLOR): D-4 (cas-405f) — match the
                    // director's config.json color so the inbox bubble isn't misattributed.
                    let _ = self
                        .deliver_to_worker(
                            &actual,
                            super::teams::DIRECTOR_AGENT_NAME,
                            &prompt,
                            None,
                            Some(super::teams::DIRECTOR_AGENT_COLOR),
                            None,
                            None,
                        )
                        .await;
                }
            }
            ClientMessage::GetState => {
                let state = self.build_session_state();
                let msg = DaemonMessage::StateUpdate { state };
                if let Some(frame) = encode_frame(&msg) {
                    if let Some(client) = self.gui_clients.get_mut(&client_id) {
                        queue_frame(client, &frame);
                    }
                }
            }
            ClientMessage::Ping => {
                if let Some(frame) = encode_frame(&DaemonMessage::Pong) {
                    if let Some(client) = self.gui_clients.get_mut(&client_id) {
                        queue_frame(client, &frame);
                    }
                }
            }
            ClientMessage::Interrupt => {
                let _ = self.app.mux.interrupt_focused().await;
            }
            ClientMessage::SpawnShell { name, shell } => {
                self.pending_spawns
                    .push_back(PendingSpawn::Shell { name, shell });
            }
            ClientMessage::KillShell { name } => {
                self.pending_spawns
                    .push_back(PendingSpawn::KillShell { name });
            }
        }
    }

    /// Build a SessionState snapshot from current daemon state.
    pub(super) fn build_session_state(&self) -> SessionState {
        let focused = self.app.mux.focused().map(|p| p.id().to_string());
        let mut panes = Vec::new();

        // Add supervisor
        let sup_name = self.app.supervisor_name().to_string();
        let sup_exited = self.app.mux.get(&sup_name).is_some_and(|p| p.has_exited());
        panes.push(PaneInfo {
            id: sup_name.clone(),
            kind: PaneKind::Supervisor,
            focused: focused.as_deref() == Some(&sup_name),
            title: sup_name.clone(),
            exited: sup_exited,
        });

        // Add workers
        for name in self.app.worker_names() {
            let exited = self.app.mux.get(name).is_some_and(|p| p.has_exited());
            panes.push(PaneInfo {
                id: name.clone(),
                kind: PaneKind::Worker,
                focused: focused.as_deref() == Some(name.as_str()),
                title: name.clone(),
                exited,
            });
        }

        SessionState {
            focused_pane: focused,
            panes,
            epic_id: self.app.epic_state().epic_id().map(|s| s.to_string()),
            epic_title: self.app.epic_state().epic_title().map(|s| s.to_string()),
            cols: self.cols,
            rows: self.rows,
        }
    }

    /// Build scrollback buffers for all panes from the ring buffers.
    pub(super) fn build_scrollback(&self) -> std::collections::HashMap<String, Vec<Vec<u8>>> {
        let mut scrollback = std::collections::HashMap::new();
        for (pane_id, buffer) in &self.pane_buffers {
            let bytes = buffer.as_bytes();
            if !bytes.is_empty() {
                scrollback.insert(pane_id.clone(), vec![bytes]);
            }
        }
        scrollback
    }

    /// Resolve "supervisor" to the actual pane name.
    pub(super) fn resolve_pane_name(&self, name: &str) -> String {
        if name == "supervisor" {
            self.app.supervisor_name().to_string()
        } else {
            name.to_string()
        }
    }

    /// Broadcast a DaemonMessage to all connected GUI and WebSocket clients.
    pub(super) fn broadcast_daemon_message(&mut self, msg: &DaemonMessage) {
        if !self.gui_clients.is_empty() {
            if let Some(frame) = encode_frame(msg) {
                for client in self.gui_clients.values_mut() {
                    queue_frame(client, &frame);
                }
            }
        }
        if !self.ws_clients.is_empty() {
            self.ws_broadcast(msg);
        }
    }

    /// Broadcast a DaemonMessage to all connected GUI clients only.
    #[allow(dead_code)]
    fn gui_broadcast(&mut self, msg: &DaemonMessage) {
        if let Some(frame) = encode_frame(msg) {
            for client in self.gui_clients.values_mut() {
                queue_frame(client, &frame);
            }
        }
    }

    /// Recalculate effective pane sizes after a GUI client disconnects.
    /// The disconnected client may have been the smallest, so panes can expand.
    fn reconcile_after_gui_disconnect(&mut self) {
        // Collect all pane IDs that any remaining GUI or WS client has sizes for
        let mut pane_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for client in self.gui_clients.values() {
            pane_ids.extend(client.pane_sizes.keys().cloned());
        }
        for client in self.ws_clients.values() {
            pane_ids.extend(client.pane_sizes.keys().cloned());
        }
        // Also include panes with TUI or web sizes
        pane_ids.extend(self.tui_pane_sizes.keys().cloned());
        pane_ids.extend(self.web_pane_sizes.keys().cloned());

        for pane_id in pane_ids {
            self.apply_effective_pane_size(&pane_id);
        }
    }

    /// Calculate and apply the effective size for a pane across all client types
    /// (TUI, GUI, web). Uses the smallest dimensions so every viewer can display
    /// the content without clipping.
    pub(super) fn apply_effective_pane_size(&mut self, pane_id: &str) {
        let mut min_cols = u16::MAX;
        let mut min_rows = u16::MAX;
        let mut found = false;

        // TUI layout allocation
        if let Some(&(cols, rows)) = self.tui_pane_sizes.get(pane_id) {
            if cols > 0 && rows > 0 {
                min_cols = min_cols.min(cols);
                min_rows = min_rows.min(rows);
                found = true;
            }
        }

        // All GUI clients
        for client in self.gui_clients.values() {
            if let Some(&(cols, rows)) = client.pane_sizes.get(pane_id) {
                if cols > 0 && rows > 0 {
                    min_cols = min_cols.min(cols);
                    min_rows = min_rows.min(rows);
                    found = true;
                }
            }
        }

        // All WS clients
        for client in self.ws_clients.values() {
            if let Some(&(cols, rows)) = client.pane_sizes.get(pane_id) {
                if cols > 0 && rows > 0 {
                    min_cols = min_cols.min(cols);
                    min_rows = min_rows.min(rows);
                    found = true;
                }
            }
        }

        // Web viewers
        if let Some(&(cols, rows)) = self.web_pane_sizes.get(pane_id) {
            if cols > 0 && rows > 0 {
                min_cols = min_cols.min(cols);
                min_rows = min_rows.min(rows);
                found = true;
            }
        }

        if !found {
            return;
        }

        if let Some(pane) = self.app.mux.get_mut(pane_id) {
            if pane.cols() != min_cols || pane.rows() != min_rows {
                let _ = pane.resize(min_rows, min_cols);
                tracing::info!(
                    "Pane '{}' resized to {}x{} (effective minimum across all clients)",
                    pane_id,
                    min_cols,
                    min_rows,
                );
            }
        }
    }

    /// Snapshot current mux pane sizes into tui_pane_sizes after a TUI layout resize.
    /// Then re-apply effective sizes for panes that have GUI or web constraints.
    pub(super) fn snapshot_tui_pane_sizes_and_reconcile(&mut self) {
        // Collect current mux-allocated sizes
        self.tui_pane_sizes.clear();
        let sup_name = self.app.supervisor_name().to_string();
        if let Some(pane) = self.app.mux.get(&sup_name) {
            self.tui_pane_sizes
                .insert(sup_name.clone(), (pane.cols(), pane.rows()));
        }
        for name in self.app.worker_names() {
            if let Some(pane) = self.app.mux.get(name) {
                self.tui_pane_sizes
                    .insert(name.clone(), (pane.cols(), pane.rows()));
            }
        }

        // Re-apply effective sizes for any panes that have GUI or web constraints
        let pane_ids: Vec<String> = self.tui_pane_sizes.keys().cloned().collect();
        for pane_id in pane_ids {
            let has_gui = self
                .gui_clients
                .values()
                .any(|c| c.pane_sizes.contains_key(&pane_id));
            let has_ws = self
                .ws_clients
                .values()
                .any(|c| c.pane_sizes.contains_key(&pane_id));
            let has_web = self.web_pane_sizes.contains_key(&pane_id);
            if has_gui || has_ws || has_web {
                self.apply_effective_pane_size(&pane_id);
            }
        }
    }
}
