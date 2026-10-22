use crate::ui::factory::daemon::imports::*;

impl FactoryDaemon {
    /// A lost last TUI is an ended factory unless that client explicitly
    /// detached. Never infer ownership from a global process-name search.
    pub(super) fn disconnect_tui_clients(&mut self, disconnected: &[usize]) {
        let mut unexpected = false;
        for id in disconnected {
            if let Some(mut client) = self.clients.remove(id) {
                // A detach frame can arrive between the input pass and a
                // failed output write. Read its queued bytes before retiring
                // the socket, so that race still preserves intentional detach.
                let mut buf = [0u8; 4096];
                while let Ok(n) = client.stream.read(&mut buf) {
                    if n == 0 { break; }
                    client.input_buf.extend_from_slice(&buf[..n]);
                }
                let (_, events) = Self::parse_client_input(&mut client.input_buf);
                let detached = client.detaching
                    || events.iter().any(|event| matches!(event, ControlEvent::Detach));
                unexpected |= !detached;
            }
            if self.owner_client_id == Some(*id) {
                self.owner_client_id = None;
            }
        }
        if unexpected && self.clients.is_empty() {
            self.shutdown.store(true, Ordering::Relaxed);
        }
        if !disconnected.is_empty() {
            self.recalibrate_after_disconnect();
        }
    }

    /// Broadcast output only to clients with a specific view mode
    pub(super) fn broadcast_output_to(&mut self, data: &[u8], mode: ClientViewMode) {
        let mut disconnected = Vec::new();

        for (id, client) in self.clients.iter_mut() {
            if client.view_mode != mode {
                continue;
            }

            if !data.is_empty() {
                if client.output_buf.len() + data.len() > MAX_CLIENT_OUTPUT_BYTES {
                    client.output_buf.clear();
                    client.needs_full_redraw = true;
                }
                client.output_buf.extend(data);
            }

            if client.output_buf.is_empty() {
                continue;
            }

            let (front, back) = client.output_buf.as_slices();
            let chunk = if !front.is_empty() { front } else { back };
            if chunk.is_empty() {
                continue;
            }

            match client.stream.write(chunk) {
                Ok(0) => {
                    disconnected.push(*id);
                }
                Ok(n) => {
                    client.output_buf.drain(..n);
                }
                Err(e) => match e.kind() {
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => {}
                    _ => disconnected.push(*id),
                },
            }
        }

        self.disconnect_tui_clients(&disconnected);
    }

    /// Flush pending output to all clients without adding new data
    pub(super) fn flush_client_output(&mut self) {
        let mut disconnected = Vec::new();

        for (id, client) in self.clients.iter_mut() {
            if client.output_buf.is_empty() {
                continue;
            }

            let (front, back) = client.output_buf.as_slices();
            let chunk = if !front.is_empty() { front } else { back };
            if chunk.is_empty() {
                continue;
            }

            match client.stream.write(chunk) {
                Ok(0) => {
                    disconnected.push(*id);
                }
                Ok(n) => {
                    client.output_buf.drain(..n);
                }
                Err(e) => match e.kind() {
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => {}
                    _ => disconnected.push(*id),
                },
            }
        }

        self.disconnect_tui_clients(&disconnected);
    }

    /// Get the input owner's terminal dimensions for a view mode.
    ///
    /// A detached launch client may stay connected at 80x24 after an operator
    /// attaches from a larger terminal. It remains a viewer, not a geometry
    /// constraint. If the owner has gone, use the smallest remaining client
    /// until another client takes ownership.
    pub(super) fn dims_for_mode(&self, mode: ClientViewMode) -> (u16, u16) {
        if let Some(owner) = self.owner_client_id.and_then(|id| self.clients.get(&id)) {
            if owner.view_mode == mode && owner.client_cols > 0 && owner.client_rows > 0 {
                return (owner.client_cols, owner.client_rows);
            }
        }
        let mut min_cols: u16 = u16::MAX;
        let mut min_rows: u16 = u16::MAX;
        let mut found = false;
        for client in self.clients.values() {
            if client.view_mode == mode && client.client_cols > 0 && client.client_rows > 0 {
                min_cols = min_cols.min(client.client_cols);
                min_rows = min_rows.min(client.client_rows);
                found = true;
            }
        }
        if found { (min_cols, min_rows) } else { (0, 0) }
    }

    /// Recalibrate render dimensions after a client disconnects.
    /// If the disconnected client was the smallest, the TUI can now expand.
    pub(super) fn recalibrate_after_disconnect(&mut self) {
        let has_full = self
            .clients
            .values()
            .any(|c| c.view_mode == ClientViewMode::Full);
        if !has_full {
            // The dashboard that owned pane geometry is gone (cas-37f8), so the
            // attached viewers now decide. Without this the last dashboard
            // layout would keep a remote-only session pinned to a size no
            // viewer asked for.
            let pane_ids: Vec<String> = self.dashboard_pane_ids();
            for pane_id in pane_ids {
                self.apply_effective_pane_size(&pane_id);
            }
        }
        if has_full {
            let (cols, rows) = self.dims_for_mode(ClientViewMode::Full);
            if cols > 0 && rows > 0 && (cols != self.cols || rows != self.rows) {
                self.cols = cols;
                self.rows = rows;
                let _ = self.app.handle_resize(cols, rows);
                for client in self.clients.values_mut() {
                    if client.view_mode == ClientViewMode::Full {
                        client.needs_full_redraw = true;
                    }
                }
            }
        }

        let has_compact = self
            .clients
            .values()
            .any(|c| c.view_mode == ClientViewMode::Compact);
        if has_compact {
            let (cc, cr) = self.dims_for_mode(ClientViewMode::Compact);
            if cc > 0 && cr > 0 && (cc != self.compact_cols || cr != self.compact_rows) {
                self.compact_cols = cc;
                self.compact_rows = cr;
                for client in self.clients.values_mut() {
                    if client.view_mode == ClientViewMode::Compact {
                        client.needs_full_redraw = true;
                    }
                }
            }
        }
    }
}
