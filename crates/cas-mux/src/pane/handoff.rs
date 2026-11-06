//! Pane state held across a same-PID daemon exec. Descriptors remain owned by
//! the old image until exec; adoption clones them so rollback retains custody.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyHandoff {
    pub master_fd: i32,
    pub child_pid: u32,
    pub pgid: u32,
    pub startup_brief: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneHandoff {
    pub id: String,
    pub kind: PaneKind,
    pub harness: SupervisorCli,
    pub title: String,
    pub color: Option<String>,
    pub focused: bool,
    pub rows: u16,
    pub cols: u16,
    pub pty: Option<PtyHandoff>,
    pub snapshot: TerminalSnapshot,
    pub terminal: Vec<u8>,
    pub scroll_offset: u32,
    pub recording_path: Option<PathBuf>,
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<String>,
    pub total_bytes_received: u64,
    pub turn_in_flight: bool,
    pub harness_session_id: Option<String>,
    pub turn_events_byte_offset: Option<u64>,
    pub composer_dirty: bool,
    pub draft_chars: usize,
    pub in_alt_screen: bool,
    pub mouse_modes: u8,
    pub has_hyperlinks: bool,
    pub partial_esc: Vec<u8>,
    pub partial_osc8: Vec<u8>,
}

impl Pane {
    #[cfg(unix)]
    pub async fn prepare_handoff(&mut self) -> Result<PaneHandoff> {
        use std::sync::atomic::Ordering;
        let (events, pty) = match &mut self.backend {
            PaneBackend::Pty(pty) => {
                let events = pty.quiesce().await?;
                let state = PtyHandoff {
                    master_fd: pty.master_fd().ok_or_else(|| Error::pty("PTY master unavailable"))?,
                    child_pid: pty.child_pid().ok_or_else(|| Error::pty("PTY child unavailable"))?,
                    pgid: pty.process_group_id().ok_or_else(|| Error::pty("PTY process group unavailable"))?,
                    startup_brief: pty.startup_brief_path(),
                };
                (events, Some(state))
            }
            PaneBackend::None => (Vec::new(), None),
        };
        for event in events {
            if let PtyEvent::Output(data) = event {
                self.total_bytes_received += data.len() as u64;
                self.feed(&data)?;
                self.record_output(&data).await?;
            }
        }
        // Every byte read before the cut is now in this terminal. Unread bytes
        // stay in the kernel; the new reader consumes them after adoption.
        if let PaneBackend::Pty(pty) = &self.backend {
            pty.flush_input().await?;
        }
        let recording_path = self.stop_recording().await?;
        Ok(PaneHandoff {
            id: self.id.clone(), kind: self.kind.clone(), harness: self.harness,
            title: self.title.clone(), color: self.color.clone(), focused: self.focused,
            rows: self.rows, cols: self.cols, pty,
            snapshot: self.get_full_snapshot()?,
            terminal: self.terminal.handoff_bytes().map_err(|e| Error::terminal(e.to_string()))?,
            scroll_offset: self.scroll_offset(), recording_path,
            exited: self.exited, exit_code: self.exit_code, exit_signal: self.exit_signal.clone(),
            total_bytes_received: self.total_bytes_received,
            turn_in_flight: self.turn_in_flight.load(Ordering::Acquire),
            harness_session_id: self.harness_session_id(),
            turn_events_byte_offset: *self.turn_events_byte_offset.lock().unwrap_or_else(|p| p.into_inner()),
            composer_dirty: self.composer_dirty.load(Ordering::Acquire),
            draft_chars: self.draft_chars.load(Ordering::Acquire),
            in_alt_screen: self.in_alt_screen, mouse_modes: self.mouse_modes,
            has_hyperlinks: self.has_hyperlinks,
            partial_esc: self.partial_esc.clone(), partial_osc8: self.partial_osc8.clone(),
        })
    }

    /// Rebuild the terminal and stage handles without consuming PTY output.
    #[cfg(unix)]
    pub fn from_handoff(state: &PaneHandoff) -> Result<Self> {
        use std::os::fd::FromRawFd;
        use std::sync::atomic::Ordering;
        if state.rows == 0 || state.cols == 0 {
            return Err(Error::pty("Invalid inherited pane geometry"));
        }
        let backend = match &state.pty {
            Some(pty) => {
                let fd = unsafe { libc::fcntl(pty.master_fd, libc::F_DUPFD_CLOEXEC, 3) };
                if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
                let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
                let mut pty = Pty::adopt_paused(fd, pty.child_pid, pty.pgid)?;
                pty.set_codex(state.harness == SupervisorCli::Codex);
                PaneBackend::Pty(pty)
            }
            None => PaneBackend::None,
        };
        let mut pane = Self::new_with_backend(&state.id, &state.title, state.kind.clone(),
            backend, state.rows, state.cols, state.harness)?;
        pane.terminal.feed(&state.terminal).map_err(|e| Error::terminal(e.to_string()))?;
        pane.terminal.scroll(-(state.scroll_offset.min(i32::MAX as u32) as i32))
            .map_err(|e| Error::terminal(e.to_string()))?;
        pane.focused = state.focused;
        pane.color = state.color.clone();
        pane.exited = state.exited;
        pane.exit_code = state.exit_code;
        pane.exit_signal = state.exit_signal.clone();
        pane.total_bytes_received = state.total_bytes_received;
        pane.turn_in_flight.store(state.turn_in_flight, Ordering::Release);
        *pane.harness_session_id.lock().unwrap_or_else(|p| p.into_inner()) = state.harness_session_id.clone();
        *pane.turn_events_byte_offset.lock().unwrap_or_else(|p| p.into_inner()) = state.turn_events_byte_offset;
        pane.composer_dirty.store(state.composer_dirty, Ordering::Release);
        pane.draft_chars.store(state.draft_chars, Ordering::Release);
        pane.in_alt_screen = state.in_alt_screen;
        pane.mouse_modes = state.mouse_modes;
        pane.has_hyperlinks = state.has_hyperlinks;
        pane.partial_esc = state.partial_esc.clone();
        pane.partial_osc8 = state.partial_osc8.clone();
        pane.user_scrolled = state.scroll_offset != 0;
        Ok(pane)
    }

    pub fn resume_after_handoff(&mut self) -> Result<()> {
        if let PaneBackend::Pty(pty) = &mut self.backend {
            pty.resume()?;
            pty.resize(self.rows, self.cols)?;
        }
        Ok(())
    }
}
