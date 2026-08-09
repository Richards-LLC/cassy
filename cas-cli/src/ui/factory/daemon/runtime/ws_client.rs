use crate::ui::factory::daemon::imports::*;
use crate::ui::factory::protocol::{ClientMessage, DaemonMessage};
use futures_util::{FutureExt, SinkExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Encode a DaemonMessage as a WebSocket Binary frame (raw JSON, no length prefix).
fn ws_encode(msg: &DaemonMessage) -> Option<WsMessage> {
    serde_json::to_vec(msg).ok().map(WsMessage::Binary)
}

impl FactoryDaemon {
    /// Accept new WebSocket client connections (non-blocking).
    pub(super) async fn accept_ws_clients(&mut self) -> bool {
        let listener = match self.ws_listener {
            Some(ref listener) => listener,
            None => return false,
        };

        let mut any_new = false;
        // Non-blocking: poll accept once per tick using now_or_never
        while let Some(Ok((tcp_stream, addr))) = listener.accept().now_or_never() {
            tracing::info!("WS TCP connection from {}", addr);

            // Perform the WebSocket handshake
            let ws_stream = match tokio_tungstenite::accept_async(tcp_stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    tracing::warn!("WS handshake failed from {}: {}", addr, e);
                    continue;
                }
            };

            let client_id = self.next_ws_client_id;
            self.next_ws_client_id += 1;

            let (mut sink, stream) = futures_util::StreamExt::split(ws_stream);

            // Build and send Welcome message
            let state = self.build_session_state();
            let scrollback = self.build_scrollback();
            let welcome = DaemonMessage::Welcome {
                session_name: self.session_name.clone(),
                state,
                scrollback: Some(scrollback),
                protocol_version: crate::ui::factory::protocol::PROTOCOL_VERSION,
                capabilities: crate::ui::factory::protocol::daemon_capabilities(),
            };

            if let Some(frame) = ws_encode(&welcome) {
                if let Err(e) = sink.send(frame).await {
                    tracing::warn!("WS client {} welcome send failed: {}", client_id, e);
                    continue;
                }
            }

            tracing::info!("WS client {} connected", client_id);
            self.ws_clients.insert(
                client_id,
                WsConnection {
                    sink,
                    stream,
                    pane_sizes: HashMap::new(),
                },
            );
            any_new = true;
        }
        any_new
    }

    /// Process input from all WebSocket clients, returning whether any activity occurred.
    pub(super) async fn process_ws_client_input(&mut self) -> bool {
        let client_ids: Vec<usize> = self.ws_clients.keys().copied().collect();
        let mut disconnected = Vec::new();
        let mut messages: Vec<(usize, ClientMessage)> = Vec::new();

        for client_id in client_ids {
            if let Some(client) = self.ws_clients.get_mut(&client_id) {
                // Try to receive messages without blocking (poll once)
                loop {
                    match futures_util::StreamExt::next(&mut client.stream).now_or_never() {
                        Some(Some(Ok(msg))) => match msg {
                            WsMessage::Binary(data) => {
                                match serde_json::from_slice::<ClientMessage>(&data) {
                                    Ok(client_msg) => messages.push((client_id, client_msg)),
                                    Err(e) => {
                                        tracing::warn!(
                                            "WS client {} sent invalid message: {}",
                                            client_id,
                                            e
                                        );
                                    }
                                }
                            }
                            WsMessage::Text(text) => {
                                match serde_json::from_str::<ClientMessage>(&text) {
                                    Ok(client_msg) => messages.push((client_id, client_msg)),
                                    Err(e) => {
                                        tracing::warn!(
                                            "WS client {} sent invalid text message: {}",
                                            client_id,
                                            e
                                        );
                                    }
                                }
                            }
                            WsMessage::Close(_) => {
                                disconnected.push(client_id);
                                break;
                            }
                            WsMessage::Ping(_) | WsMessage::Pong(_) => {
                                // tungstenite handles ping/pong automatically
                            }
                            _ => {}
                        },
                        Some(Some(Err(_))) => {
                            disconnected.push(client_id);
                            break;
                        }
                        Some(None) => {
                            // Stream ended
                            disconnected.push(client_id);
                            break;
                        }
                        None => {
                            // No message ready (would block)
                            break;
                        }
                    }
                }
            }
        }

        for id in &disconnected {
            let had_sizes = self
                .ws_clients
                .get(id)
                .is_some_and(|c| !c.pane_sizes.is_empty());
            self.ws_clients.remove(id);
            tracing::info!("WS client {} disconnected", id);
            if had_sizes {
                self.reconcile_after_ws_disconnect();
            }
        }

        let had_activity = !messages.is_empty();
        for (client_id, msg) in messages {
            self.handle_ws_message(client_id, msg).await;
        }
        had_activity
    }

    /// Flush pending output to all WebSocket clients.
    pub(super) async fn flush_ws_client_output(&mut self) {
        let mut disconnected = Vec::new();

        let client_ids: Vec<usize> = self.ws_clients.keys().copied().collect();
        for client_id in client_ids {
            if let Some(client) = self.ws_clients.get_mut(&client_id) {
                if let Err(_) = client.sink.flush().await {
                    disconnected.push(client_id);
                }
            }
        }

        for id in disconnected {
            let had_sizes = self
                .ws_clients
                .get(&id)
                .is_some_and(|c| !c.pane_sizes.is_empty());
            self.ws_clients.remove(&id);
            tracing::info!("WS client {} disconnected (write error)", id);
            if had_sizes {
                self.reconcile_after_ws_disconnect();
            }
        }
    }

    /// Broadcast a DaemonMessage to all connected WebSocket clients.
    pub(super) fn ws_broadcast(&mut self, msg: &DaemonMessage) {
        if let Some(frame) = ws_encode(msg) {
            for client in self.ws_clients.values_mut() {
                // feed() buffers without flushing — flush happens in flush_ws_client_output()
                let _ = client.sink.feed(frame.clone()).now_or_never();
            }
        }
    }

    /// Forward per-pane PTY output to all connected WebSocket clients.
    pub(super) fn forward_pane_output_to_ws(&mut self, pane_id: &str, data: &[u8]) {
        if self.ws_clients.is_empty() || data.is_empty() {
            return;
        }
        let msg = DaemonMessage::Output {
            pane_id: pane_id.to_string(),
            data: data.to_vec(),
        };
        self.ws_broadcast(&msg);
    }

    /// Handle a single ClientMessage from a WebSocket client.
    /// Reuses handle_gui_message logic but routes responses to the WS client.
    async fn handle_ws_message(&mut self, client_id: usize, msg: ClientMessage) {
        match msg {
            ClientMessage::Attach { .. } => {
                let state = self.build_session_state();
                let scrollback = self.build_scrollback();
                let welcome = DaemonMessage::Welcome {
                    session_name: self.session_name.clone(),
                    state,
                    scrollback: Some(scrollback),
                    protocol_version: crate::ui::factory::protocol::PROTOCOL_VERSION,
                    capabilities: crate::ui::factory::protocol::daemon_capabilities(),
                };
                if let Some(frame) = ws_encode(&welcome) {
                    if let Some(client) = self.ws_clients.get_mut(&client_id) {
                        let _ = client.sink.feed(frame).now_or_never();
                    }
                }
            }
            ClientMessage::Detach => {
                if let Some(frame) = ws_encode(&DaemonMessage::Detached) {
                    if let Some(client) = self.ws_clients.get_mut(&client_id) {
                        let _ = client.sink.send(frame).await;
                    }
                }
                let had_sizes = self
                    .ws_clients
                    .get(&client_id)
                    .is_some_and(|c| !c.pane_sizes.is_empty());
                self.ws_clients.remove(&client_id);
                tracing::info!("WS client {} detached", client_id);
                if had_sizes {
                    self.reconcile_after_ws_disconnect();
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
                    "WS client {} reported global resize: {}x{}",
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
                if let Some(client) = self.ws_clients.get_mut(&client_id) {
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
                            request_id: None,
                            isolate: false,
                            spec,
                            // cas-6913: task_id pre-assignment is MCP-only for
                            // now — the WS client protocol has no task_id field.
                            task_id: None,
                        });
                    }
                } else {
                    self.app.spawning_count += names.len();
                    for (i, name) in names.into_iter().enumerate() {
                        let spec = specs.get(i).cloned().flatten();
                        self.pending_spawns.push_back(PendingSpawn::Named {
                            request_id: None,
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
                    request_id: None,
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
                            None,
                        )
                        .await;
                }
            }
            ClientMessage::InterruptPane { pane_id } => {
                if let Err(error) = self.interrupt_pane_turn(&pane_id).await
                    && let Some(frame) = ws_encode(&DaemonMessage::Error {
                        message: format!("targeted interrupt failed: {error}"),
                    })
                    && let Some(client) = self.ws_clients.get_mut(&client_id)
                {
                    let _ = client.sink.feed(frame).now_or_never();
                }
            }
            ClientMessage::SendMessage {
                target,
                text,
                summary,
                urgent,
                attribution,
            } => {
                if let Err(error) = self.enqueue_attributed_message(
                    &target,
                    &text,
                    summary.as_deref(),
                    urgent,
                    &attribution,
                ) && let Some(frame) = ws_encode(&DaemonMessage::Error {
                    message: format!("semantic message enqueue failed: {error}"),
                }) && let Some(client) = self.ws_clients.get_mut(&client_id)
                {
                    let _ = client.sink.feed(frame).now_or_never();
                }
            }
            ClientMessage::GetState => {
                let state = self.build_session_state();
                let msg = DaemonMessage::StateUpdate { state };
                if let Some(frame) = ws_encode(&msg) {
                    if let Some(client) = self.ws_clients.get_mut(&client_id) {
                        let _ = client.sink.feed(frame).now_or_never();
                    }
                }
            }
            ClientMessage::Ping => {
                if let Some(frame) = ws_encode(&DaemonMessage::Pong) {
                    if let Some(client) = self.ws_clients.get_mut(&client_id) {
                        let _ = client.sink.feed(frame).now_or_never();
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

    /// Recalculate effective pane sizes after a WS client disconnects.
    fn reconcile_after_ws_disconnect(&mut self) {
        let mut pane_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for client in self.ws_clients.values() {
            pane_ids.extend(client.pane_sizes.keys().cloned());
        }
        for client in self.gui_clients.values() {
            pane_ids.extend(client.pane_sizes.keys().cloned());
        }
        pane_ids.extend(self.tui_pane_sizes.keys().cloned());
        pane_ids.extend(self.web_pane_sizes.keys().cloned());

        for pane_id in pane_ids {
            self.apply_effective_pane_size(&pane_id);
        }
    }
}
