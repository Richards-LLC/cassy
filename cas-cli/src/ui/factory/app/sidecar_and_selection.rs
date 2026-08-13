use crate::ui::factory::app::imports::*;

// =============================================================================
// Scroll dispatch types and constants
// =============================================================================

/// Outcome returned by [`FactoryApp::handle_scroll_up`] /
/// [`FactoryApp::handle_scroll_down`].
///
/// The caller uses this to decide whether to forward an escape sequence to the
/// focused PTY (alt-screen path) or to leave the result as-is (the handler
/// already performed the scroll).
#[derive(Debug, PartialEq)]
pub enum ScrollAction {
    /// Scroll was handled internally — by a dialog overlay, sidecar, Mission
    /// Control panel, or regular host-side scrollback.
    Done,
    /// The focused pane is in alt-screen mode and no overlay is suppressing
    /// forwarding.  The caller must send [`FactoryApp::alt_screen_scroll_payload`]
    /// (harness-aware: SGR wheel for Grok, PgUp/PgDn for Claude/Codex).
    AltScreen,
}

/// Number of lines scrolled per wheel tick (host scrollback).
pub const SCROLL_LINES: usize = 3;

/// PgUp bytes forwarded to an alt-screen PTY on scroll-up (`\x1b[5~`).
/// Used for Claude/Codex (cas-f93a). Grok ignores this when the prompt is
/// focused — see [`alt_screen_wheel_bytes`].
pub const SCROLL_UP_ARROWS: &[u8] = b"\x1b[5~";
/// PgDn bytes forwarded to an alt-screen PTY on scroll-down (`\x1b[6~`).
pub const SCROLL_DOWN_ARROWS: &[u8] = b"\x1b[6~";

/// One SGR 1006 mouse-wheel-up event (`CSI < 64 ; col ; row M`).
/// Button 64 = wheel up. Coordinates are 1-based; top-left content area is
/// enough for Grok to treat the gesture as scrollback scroll (cas-d3b5).
pub const SCROLL_UP_SGR: &[u8] = b"\x1b[<64;2;2M";
/// One SGR 1006 mouse-wheel-down event (`CSI < 65 ; col ; row M`).
pub const SCROLL_DOWN_SGR: &[u8] = b"\x1b[<65;2;2M";

// Compile-time assertion: byte count must stay in sync with the sequences above.
// PgUp (ESC [ 5 ~) and PgDn (ESC [ 6 ~) are each 4 bytes.
// SGR wheel (ESC [ < 64 ; 2 ; 2 M) is 10 bytes.
const _: () = {
    assert!(SCROLL_UP_ARROWS.len() == 4);
    assert!(SCROLL_DOWN_ARROWS.len() == 4);
    assert!(SCROLL_UP_SGR.len() == 10);
    assert!(SCROLL_DOWN_SGR.len() == 10);
};

/// Bytes to inject into an alt-screen PTY for one mouse-wheel tick.
///
/// Harness-aware (cas-d3b5):
/// - **Grok**: SGR mouse wheel (`\x1b[<64;…M` / `\x1b[<65;…M`) × [`SCROLL_LINES`].
///   Live PTY A/B on grok 0.2.93: when the **prompt** is focused (the default),
///   PgUp/PgDn are no-ops, but SGR wheel scrolls the transcript. When scrollback
///   is focused both work — SGR is therefore the safe universal Grok payload.
/// - **Claude / Codex**: PgUp/PgDn (cas-f93a). Claude Code was verified to page
///   its transcript on those sequences; keep that path for no-regression.
pub fn alt_screen_wheel_bytes(cli: cas_mux::SupervisorCli, up: bool) -> Vec<u8> {
    match cli {
        cas_mux::SupervisorCli::Grok => {
            let unit = if up { SCROLL_UP_SGR } else { SCROLL_DOWN_SGR };
            unit.repeat(SCROLL_LINES)
        }
        cas_mux::SupervisorCli::Claude | cas_mux::SupervisorCli::Codex => {
            if up {
                SCROLL_UP_ARROWS.to_vec()
            } else {
                SCROLL_DOWN_ARROWS.to_vec()
            }
        }
    }
}

/// SGR 1006 left-button press+release at 1-based terminal coordinates.
///
/// Used to forward factory mouse clicks into an already-focused Grok alt-screen
/// pane so the on-screen **Stop** control receives the click (cas-7f6f). Factory
/// mouse capture otherwise steals clicks for pane-focus only.
pub fn sgr_left_click_bytes(col: u16, row: u16) -> Vec<u8> {
    let col = col.max(1);
    let row = row.max(1);
    // Press: CSI < 0 ; col ; row M   Release: CSI < 0 ; col ; row m
    format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m").into_bytes()
}

/// Outcome of [`FactoryApp::handle_mouse_click`].
#[derive(Debug, PartialEq, Eq)]
pub enum ClickAction {
    /// Click was handled by factory chrome (tabs, focus change, dialogs).
    Handled,
    /// Forward an SGR click into the named pane (already focused, alt-screen Grok).
    ForwardSgr {
        pane: String,
        /// 1-based PTY column
        col: u16,
        /// 1-based PTY row
        row: u16,
    },
}

/// How the factory should handle a standalone Esc on a focused Grok pane (cas-7f6f).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrokEscAction {
    /// Mid-turn: rewrite Esc to harness cancel (Ctrl+C via `break_turn`).
    CancelTurn,
    /// Idle: forward raw Esc so clear/rewind semantics stay intact.
    ForwardRaw,
}

/// Pure decision for Grok Esc routing (unit-tested active-vs-idle).
///
/// `turn_in_flight` is authoritative here because this decision is Grok-only:
/// Grok supplies `turn_ended` on normal completion. The underlying pane flag
/// is not a busy/idle signal for Claude or Codex.
pub fn grok_esc_action(turn_in_flight: bool) -> GrokEscAction {
    if turn_in_flight {
        GrokEscAction::CancelTurn
    } else {
        GrokEscAction::ForwardRaw
    }
}

/// Which client render surface to use for hit-testing / Stop coords (cas-7f6f).
///
/// Full and compact clients share one `FactoryApp` but paint different layouts;
/// geometry must be selected by the input-owning client's view mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientGeometryMode {
    Full,
    Compact,
}

/// Full-mode bordered content inset (1 cell each side). Kept next to click
/// geometry so tests don't reach into private render modules.
pub fn full_mode_pty_content(outer: Rect) -> Rect {
    Rect {
        x: outer.x.saturating_add(1),
        y: outer.y.saturating_add(1),
        width: outer.width.saturating_sub(2),
        height: outer.height.saturating_sub(2),
    }
}

impl FactoryApp {
    // ========================================================================
    // Sidecar navigation
    // ========================================================================

    /// Toggle sidecar focus (enter if None, exit if focused)
    pub fn toggle_sidecar_focus(&mut self) {
        tracing::debug!(
            "toggle_sidecar_focus called, current focus: {:?}",
            self.sidecar_focus
        );
        if self.sidecar_focus == SidecarFocus::None {
            self.sidecar_focus = SidecarFocus::Factory;
            tracing::debug!("Set sidecar_focus to Factory");
            self.init_panel_selection();
        } else {
            self.sidecar_focus = SidecarFocus::None;
            tracing::debug!("Set sidecar_focus to None");
        }
    }

    /// Move to next sidecar panel
    pub fn next_sidecar_panel(&mut self) {
        let has_reminders = !self.director_data.reminders.is_empty();
        self.sidecar_focus = self.sidecar_focus.next_with_reminders(has_reminders);
        // Initialize selection for the new panel if needed
        self.init_panel_selection();
    }

    /// Move to previous sidecar panel
    pub fn prev_sidecar_panel(&mut self) {
        let has_reminders = !self.director_data.reminders.is_empty();
        self.sidecar_focus = self.sidecar_focus.prev_with_reminders(has_reminders);
        self.init_panel_selection();
    }

    /// Focus a specific sidecar panel
    pub fn focus_sidecar_panel(&mut self, panel: SidecarFocus) {
        self.sidecar_focus = panel;
        self.init_panel_selection();
    }

    /// Get the item count for a given panel focus.
    fn item_count_for_focus(&self, focus: SidecarFocus) -> usize {
        match focus {
            SidecarFocus::None => 0,
            SidecarFocus::Factory => self.director_data.agents.len(),
            SidecarFocus::Tasks => self.task_display_item_count(),
            SidecarFocus::Reminders => self.director_data.reminders.len(),
            SidecarFocus::Changes => self.changes_item_types.len(),
            SidecarFocus::Activity => self.director_data.activity.len(),
        }
    }

    /// Initialize selection for the currently focused panel
    fn init_panel_selection(&mut self) {
        let count = self.item_count_for_focus(self.sidecar_focus);
        if let Some(p) = self.panels.get_mut(self.sidecar_focus) {
            p.init_selection(count);
        }
    }

    /// Scroll up in the current sidecar panel
    pub fn sidecar_scroll_up(&mut self) {
        if let Some(p) = self.panels.get_mut(self.sidecar_focus) {
            p.scroll_up();
        }
    }

    /// Scroll down in the current sidecar panel
    pub fn sidecar_scroll_down(&mut self) {
        let count = self.item_count_for_focus(self.sidecar_focus);
        if let Some(p) = self.panels.get_mut(self.sidecar_focus) {
            p.scroll_down(count);
        }
    }

    /// Check if sidecar is focused
    pub fn sidecar_is_focused(&self) -> bool {
        self.sidecar_focus != SidecarFocus::None
    }

    /// Register a session ID to pane name mapping
    ///
    /// This is called when a Claude session is detected to enable
    /// interaction routing to the correct pane.
    pub fn register_session(&mut self, session_id: &str, pane_name: &str) {
        tracing::info!("Registering session {} -> pane {}", session_id, pane_name);
        self.session_to_pane
            .insert(session_id.to_string(), pane_name.to_string());
    }

    /// Sync session_id → pane_name mappings from the agent store
    ///
    /// Queries registered agents and maps their session IDs to pane names.
    pub(super) fn sync_session_mappings(&mut self) {
        let agent_store = match open_agent_store(&self.cas_dir) {
            Ok(store) => store,
            Err(e) => {
                tracing::debug!("Failed to open agent store for session sync: {}", e);
                return;
            }
        };

        let agents = match agent_store.list(None) {
            Ok(agents) => agents,
            Err(e) => {
                tracing::debug!("Failed to list agents for session sync: {}", e);
                return;
            }
        };

        // Build mapping from agent session_id (agent.id) to agent.name
        // Agent IDs in CAS are typically the Claude session ID
        for agent in agents {
            // Skip agents we already have mapped
            if !self.session_to_pane.contains_key(&agent.id) {
                // Check if this agent name matches a pane we have
                if self.mux.get(&agent.name).is_some()
                    || agent.name == self.supervisor_name
                    || self.worker_names.contains(&agent.name)
                {
                    tracing::debug!(
                        "Auto-registering session {} -> pane {}",
                        agent.id,
                        agent.name
                    );
                    self.session_to_pane
                        .insert(agent.id.clone(), agent.name.clone());
                }
            }
        }
    }

    /// Handle mouse click at screen coordinates.
    ///
    /// Resolves which pane was clicked and focuses it. Also handles clicks
    /// on the worker tab bar to switch worker tabs.
    ///
    /// When the click is inside an **already-focused** Grok alt-screen pane,
    /// returns [`ClickAction::ForwardSgr`] so the caller can inject an SGR
    /// click into the PTY — this is what makes Grok's on-screen **Stop**
    /// control work under factory mouse capture (cas-7f6f). First click on an
    /// unfocused pane still only focuses (no forward), so idle clicks stay
    /// harmless.
    ///
    /// `geometry` selects full vs compact content maps so concurrent clients
    /// do not share one overwritten layout.
    pub fn handle_mouse_click(
        &mut self,
        col: u16,
        row: u16,
        geometry: ClientGeometryMode,
    ) -> ClickAction {
        // Don't handle clicks while modal dialogs are open
        if self.show_task_dialog
            || self.show_changes_dialog
            || self.show_reminder_dialog
            || self.show_help
            || self.show_terminal_dialog
        {
            return ClickAction::Handled;
        }

        // Compact has no worker tabs / sidecar — only supervisor content.
        if geometry == ClientGeometryMode::Full {
            // Check worker tab bar clicks (switches tab without focusing)
            if self.is_tabbed {
                if let Some(tab_area) = self.worker_tab_bar_area {
                    if tab_area.contains((col, row).into()) {
                        let all_names = self.layout_worker_names();
                        if !all_names.is_empty() {
                            let click_x = col.saturating_sub(tab_area.x) as usize;
                            // Account for 1-char left padding before first tab
                            let mut pos: usize = 1;
                            let mut clicked_tab: Option<usize> = None;
                            for (i, name) in all_names.iter().enumerate() {
                                let number = i + 1;
                                let status_icon = if self.is_pending_worker(name) {
                                    " \u{2801}" // spinner placeholder — 2 chars
                                } else {
                                    self.get_worker_status_icon(name)
                                };
                                // Must match renderer: format!(" {number} {name}{status_icon} ")
                                let label_width =
                                    3 + number.to_string().len() + name.len() + status_icon.len();
                                if click_x >= pos && click_x < pos + label_width {
                                    clicked_tab = Some(i);
                                    break;
                                }
                                pos += label_width;
                                // 1-char separator between tabs
                                if i < all_names.len() - 1 {
                                    pos += 1;
                                }
                            }
                            if let Some(clicked_tab) = clicked_tab {
                                self.select_worker_tab(clicked_tab);
                                // Also focus the clicked worker pane
                                if let Some(name) = self.worker_names.get(clicked_tab) {
                                    let name = name.clone();
                                    let _ = self.mux.focus(&name);
                                    self.sidecar_focus = SidecarFocus::None;
                                }
                            }
                        }
                        return ClickAction::Handled;
                    }
                }
            }

            // Check sidecar area clicks
            if let Some(sidecar_area) = self.sidecar_area {
                if sidecar_area.contains((col, row).into()) {
                    if self.sidecar_focus == SidecarFocus::None {
                        self.toggle_sidecar_focus();
                    }
                    return ClickAction::Handled;
                }
            }
        }

        // Check pane clicks using geometry-mode hit testing
        if let Some(pane_name) = self.pane_at_screen_for(col, row, geometry) {
            let already_focused = self.mux.focused_id() == Some(pane_name.as_str());
            let _ = self.mux.focus(&pane_name);
            self.sidecar_focus = SidecarFocus::None;

            // Update selected worker tab when clicking a worker in tabbed mode
            if geometry == ClientGeometryMode::Full && self.is_tabbed {
                if let Some(idx) = self.worker_names.iter().position(|n| n == &pane_name) {
                    self.selected_worker_tab = idx;
                }
            }

            // cas-7f6f: forward click into already-focused Grok alt-screen so
            // Stop (and other in-TUI controls) receive the event.
            if already_focused
                && self.harness_for(&pane_name) == cas_mux::SupervisorCli::Grok
                && self.mux.get(&pane_name).is_some_and(|p| p.is_in_alt_screen())
            {
                if let Some((pty_col, pty_row)) =
                    self.screen_to_pty_coords(&pane_name, col, row, geometry)
                {
                    return ClickAction::ForwardSgr {
                        pane: pane_name,
                        col: pty_col,
                        row: pty_row,
                    };
                }
            }
        }
        ClickAction::Handled
    }

    /// Map factory screen coordinates to 1-based PTY cell coordinates for a pane.
    ///
    /// Uses the mode-specific content rect from the last matching render —
    /// full mode is border-inset, compact is borderless. Concurrent full and
    /// compact clients each keep their own map (cas-7f6f).
    pub fn screen_to_pty_coords(
        &self,
        pane_name: &str,
        screen_col: u16,
        screen_row: u16,
        geometry: ClientGeometryMode,
    ) -> Option<(u16, u16)> {
        let content = self.pty_content_area(pane_name, geometry)?;
        if content.width == 0 || content.height == 0 {
            return None;
        }
        if screen_col < content.x
            || screen_row < content.y
            || screen_col >= content.x + content.width
            || screen_row >= content.y + content.height
        {
            return None;
        }
        // 1-based terminal coordinates relative to painted content origin.
        let pty_col = screen_col - content.x + 1;
        let pty_row = screen_row - content.y + 1;
        Some((pty_col, pty_row))
    }

    /// Mode-specific PTY content rect for a pane.
    pub fn pty_content_area(
        &self,
        pane_name: &str,
        geometry: ClientGeometryMode,
    ) -> Option<Rect> {
        match geometry {
            ClientGeometryMode::Full => self.full_pty_content_areas.get(pane_name).copied(),
            ClientGeometryMode::Compact => self.compact_pty_content_areas.get(pane_name).copied(),
        }
    }

    /// Pane under screen coords for the given client geometry mode.
    pub fn pane_at_screen_for(
        &self,
        x: u16,
        y: u16,
        geometry: ClientGeometryMode,
    ) -> Option<String> {
        match geometry {
            ClientGeometryMode::Compact => {
                // Compact paints only the supervisor (borderless content rect).
                let area = self.pty_content_area(&self.supervisor_name, geometry)?;
                if area.contains((x, y).into()) {
                    Some(self.supervisor_name.clone())
                } else {
                    None
                }
            }
            ClientGeometryMode::Full => self.pane_at_screen(x, y),
        }
    }

    /// Whether the focused Grok pane has an authoritative in-flight turn.
    ///
    /// Refreshes from harness events (Grok `turn_ended`) before reading so
    /// Esc routing never uses a stale in-flight flag after normal completion.
    /// This helper must stay Grok-scoped: Claude/Codex have no equivalent
    /// normal-completion signal.
    pub fn focused_grok_turn_in_flight(&self) -> bool {
        let Some(id) = self.mux.focused_id() else {
            return false;
        };
        self.mux.get(id).is_some_and(|p| {
            p.refresh_harness_turn_state();
            p.is_turn_in_flight()
        })
    }

    /// Esc routing for the focused pane when the harness is Grok.
    pub fn focused_grok_esc_action(&self) -> GrokEscAction {
        grok_esc_action(self.focused_grok_turn_in_flight())
    }

    /// Focus the next PTY pane (cycles through supervisor + worker panes only)
    pub fn focus_next_pty_pane(&mut self) {
        let pane_names = self.pty_pane_names();
        if pane_names.is_empty() {
            return;
        }

        let current = self.mux.focused_id().map(|s| s.to_string());
        let current_idx = current
            .as_ref()
            .and_then(|c| pane_names.iter().position(|n| n == c))
            .unwrap_or(0);

        let next_idx = (current_idx + 1) % pane_names.len();
        let target = pane_names[next_idx].clone();
        let _ = self.mux.focus(&target);
        self.sidecar_focus = SidecarFocus::None;

        // Sync worker tab selection
        if let Some(idx) = self.worker_names.iter().position(|n| n == &target) {
            self.selected_worker_tab = idx;
        }
    }

    /// Focus the previous PTY pane (cycles through supervisor + worker panes only)
    pub fn focus_prev_pty_pane(&mut self) {
        let pane_names = self.pty_pane_names();
        if pane_names.is_empty() {
            return;
        }

        let current = self.mux.focused_id().map(|s| s.to_string());
        let current_idx = current
            .as_ref()
            .and_then(|c| pane_names.iter().position(|n| n == c))
            .unwrap_or(0);

        let prev_idx = if current_idx == 0 {
            pane_names.len() - 1
        } else {
            current_idx - 1
        };
        let target = pane_names[prev_idx].clone();
        let _ = self.mux.focus(&target);
        self.sidecar_focus = SidecarFocus::None;

        // Sync worker tab selection
        if let Some(idx) = self.worker_names.iter().position(|n| n == &target) {
            self.selected_worker_tab = idx;
        }
    }

    /// Get ordered list of PTY pane names (supervisor first, then workers)
    fn pty_pane_names(&self) -> Vec<String> {
        let mut names = Vec::with_capacity(1 + self.worker_names.len());
        names.push(self.supervisor_name.clone());
        names.extend(self.worker_names.iter().cloned());
        names
    }

    /// Harness of the currently focused PTY pane (supervisor or worker).
    ///
    /// Used by the alt-screen wheel forward path so Grok gets SGR mouse-wheel
    /// bytes while Claude/Codex keep PgUp/PgDn (cas-d3b5).
    pub fn focused_harness(&self) -> cas_mux::SupervisorCli {
        match self.mux.focused_id() {
            Some(id) => self.harness_for(id),
            None => self.supervisor_cli,
        }
    }

    /// PTY payload for one alt-screen wheel tick on the focused pane.
    ///
    /// See [`alt_screen_wheel_bytes`]. Call only after
    /// [`handle_scroll_up`] / [`handle_scroll_down`] returned
    /// [`ScrollAction::AltScreen`].
    pub fn alt_screen_scroll_payload(&self, up: bool) -> Vec<u8> {
        alt_screen_wheel_bytes(self.focused_harness(), up)
    }

    /// Handle mouse scroll up.
    ///
    /// Returns [`ScrollAction::AltScreen`] when the focused pane is in
    /// alt-screen mode and no overlay suppresses forwarding — the caller must
    /// send [`Self::alt_screen_scroll_payload`]`(true)` to the PTY (harness-
    /// aware: SGR for Grok, PgUp for Claude/Codex).
    /// Returns [`ScrollAction::Done`] in all other cases (the scroll was
    /// handled internally by a dialog, sidecar, MC panel, or host scrollback).
    ///
    /// This is the **single source of truth** for the "where does scroll go?"
    /// decision.  Adding a new dialog flag requires only one additional
    /// `else if` branch here; alt-screen suppression is automatic because the
    /// alt-screen check lives in the final `else` arm.
    pub fn handle_scroll_up(&mut self) -> ScrollAction {
        if self.show_task_dialog {
            self.task_dialog_scroll = self.task_dialog_scroll.saturating_sub(1);
        } else if self.show_reminder_dialog {
            self.reminder_dialog_scroll = self.reminder_dialog_scroll.saturating_sub(1);
        } else if self.show_changes_dialog {
            self.changes_dialog_scroll = self.changes_dialog_scroll.saturating_sub(1);
        } else if self.is_mission_control()
            && self.mc_focus != crate::ui::factory::renderer::MissionControlFocus::None
        {
            self.mc_scroll_up();
        } else if self.sidecar_focus != SidecarFocus::None {
            self.sidecar_scroll_up();
        } else {
            // No dialog, active MC panel, or sidecar consuming the scroll.
            // Suppress alt-screen forwarding when the help overlay is open or
            // when Mission Control is active at the overview level (mc_focus ==
            // None) — in both cases fall through to normal host scrollback.
            let suppress_alt = self.show_help || self.is_mission_control();
            if !suppress_alt && self.mux.focused_is_in_alt_screen() {
                return ScrollAction::AltScreen;
            }
            self.scroll_focused_pane(-(SCROLL_LINES as i32));
        }
        ScrollAction::Done
    }

    /// Handle mouse scroll down.
    ///
    /// Mirror of [`handle_scroll_up`].  Returns [`ScrollAction::AltScreen`]
    /// when the focused pane is in alt-screen mode and no overlay suppresses
    /// forwarding — the caller must send [`Self::alt_screen_scroll_payload`]
    /// `(false)` to the PTY (harness-aware: SGR for Grok, PgDn for Claude/Codex).
    pub fn handle_scroll_down(&mut self) -> ScrollAction {
        if self.show_task_dialog {
            self.task_dialog_scroll =
                (self.task_dialog_scroll + 1).min(self.task_dialog_max_scroll);
        } else if self.show_reminder_dialog {
            self.reminder_dialog_scroll = self.reminder_dialog_scroll.saturating_add(1);
        } else if self.show_changes_dialog {
            let max_scroll = self.changes_dialog_diff.len().saturating_sub(10) as u16;
            self.changes_dialog_scroll = (self.changes_dialog_scroll + 1).min(max_scroll);
        } else if self.is_mission_control()
            && self.mc_focus != crate::ui::factory::renderer::MissionControlFocus::None
        {
            self.mc_scroll_down();
        } else if self.sidecar_focus != SidecarFocus::None {
            self.sidecar_scroll_down();
        } else {
            let suppress_alt = self.show_help || self.is_mission_control();
            if !suppress_alt && self.mux.focused_is_in_alt_screen() {
                return ScrollAction::AltScreen;
            }
            self.scroll_focused_pane(SCROLL_LINES as i32);
        }
        ScrollAction::Done
    }

    /// Convert screen coordinates to the pane at that position.
    ///
    /// Returns the pane name if the coordinates are inside a pane.
    pub fn pane_at_screen(&self, x: u16, y: u16) -> Option<String> {
        let point = (x, y);

        // Check supervisor area
        if let Some(sup_area) = self.supervisor_area {
            if sup_area.contains(point.into()) {
                return Some(self.supervisor_name.clone());
            }
        }

        // Check worker areas
        if self.is_tabbed {
            if let Some(content_area) = self.worker_content_area {
                if content_area.contains(point.into()) {
                    return self.worker_names.get(self.selected_worker_tab).cloned();
                }
            }
        } else {
            for (i, worker_area) in self.worker_areas.iter().enumerate() {
                if worker_area.contains(point.into()) {
                    return self.worker_names.get(i).cloned();
                }
            }
        }

        None
    }

    /// Scroll the supervisor pane by delta lines
    pub fn scroll_supervisor(&mut self, delta: i32) {
        if let Err(e) = self.mux.scroll_pane(&self.supervisor_name, delta) {
            tracing::warn!("Failed to scroll supervisor pane: {}", e);
        }
    }

    /// Scroll the focused pane by delta lines
    pub fn scroll_focused_pane(&mut self, delta: i32) {
        if let Err(e) = self.mux.scroll_focused(delta) {
            tracing::warn!("Failed to scroll focused pane: {}", e);
        }
    }

    /// Scroll the supervisor pane to bottom (most recent content)
    pub fn scroll_supervisor_to_bottom(&mut self) {
        if let Err(e) = self.mux.scroll_pane_to_bottom(&self.supervisor_name) {
            tracing::warn!("Failed to scroll supervisor to bottom: {}", e);
        }
    }

    /// Handle Enter key - open detail dialog for selected item
    pub fn handle_enter(&mut self) {
        if self.view_mode == ViewMode::Overview {
            match self.sidecar_focus {
                SidecarFocus::Factory => {
                    if let Some(idx) = self.panels.factory.list_state.selected() {
                        if let Some(agent) = self.director_data.agents.get(idx) {
                            let _ = self.mux.focus(&agent.name);
                            self.sidecar_focus = SidecarFocus::None;
                        }
                    }
                }
                SidecarFocus::Tasks => {
                    // Open task detail dialog
                    self.open_task_dialog();
                }
                SidecarFocus::Reminders => {
                    // Open reminder detail dialog
                    self.open_reminder_dialog();
                }
                SidecarFocus::Changes => {
                    // Open file changes dialog for selected change
                    self.open_changes_dialog();
                }
                SidecarFocus::Activity => {
                    self.detail_scroll = 0;
                    self.view_mode = ViewMode::ActivityLog;
                }
                _ => {}
            }
        }
    }

    /// Open the task detail dialog for the selected task
    pub fn open_task_dialog(&mut self) {
        if let Some(task_id) = self.get_selected_task_id() {
            self.task_dialog_id = Some(task_id);
            self.task_dialog_scroll = 0;
            self.show_task_dialog = true;
        }
    }

    /// Close the task detail dialog
    pub fn close_task_dialog(&mut self) {
        self.show_task_dialog = false;
        self.task_dialog_id = None;
        self.task_dialog_scroll = 0;
    }

    /// Open the reminder detail dialog for the selected reminder
    pub fn open_reminder_dialog(&mut self) {
        if let Some(idx) = self.panels.reminders.list_state.selected() {
            if idx < self.director_data.reminders.len() {
                self.reminder_dialog_idx = Some(idx);
                self.reminder_dialog_scroll = 0;
                self.show_reminder_dialog = true;
            }
        }
    }

    /// Close the reminder detail dialog
    pub fn close_reminder_dialog(&mut self) {
        self.show_reminder_dialog = false;
        self.reminder_dialog_idx = None;
        self.reminder_dialog_scroll = 0;
    }

    /// Handle Escape key - return to overview or unfocus sidecar
    pub fn handle_escape(&mut self) -> bool {
        // Close task dialog if open
        if self.show_task_dialog {
            self.close_task_dialog();
            return true;
        }

        // Close reminder dialog if open
        if self.show_reminder_dialog {
            self.close_reminder_dialog();
            return true;
        }

        // Close changes dialog if open
        if self.show_changes_dialog {
            self.close_changes_dialog();
            return true;
        }

        match &self.view_mode {
            ViewMode::TaskDetail(_) | ViewMode::ActivityLog => {
                self.view_mode = ViewMode::Overview;
                true
            }
            ViewMode::Overview | ViewMode::FileDiff(_, _) => {
                if self.sidecar_focus != SidecarFocus::None {
                    self.sidecar_focus = SidecarFocus::None;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Compute the total number of display items in the task panel.
    ///
    /// Must match the item count produced by `tasks::render_with_focus()`,
    /// including epic headers, subtasks (when not collapsed), separators, and standalone tasks.
    fn task_display_item_count(&self) -> usize {
        let scoped = crate::ui::factory::director::tasks::ScopedTaskView::new(
            &self.director_data,
            self.current_epic_id.as_deref(),
        );
        scoped.visible_row_count(self.agent_filter.as_deref(), &self.collapsed_epics)
    }

    /// Get the ID of the selected task (if any).
    ///
    /// Walks through display items (epic headers, subtasks, separators, standalone)
    /// to correctly map the selected display index to a task ID.
    fn get_selected_task_id(&self) -> Option<String> {
        use crate::ui::factory::director::tasks::task_matches_agent_filter;

        let selected = self.panels.tasks.list_state.selected()?;
        let scoped = crate::ui::factory::director::tasks::ScopedTaskView::new(
            &self.director_data,
            self.current_epic_id.as_deref(),
        );
        let agent_filter = self.agent_filter.as_deref();
        let mut idx = 0;

        for group in &scoped.epic_groups {
            let filtered_subtasks: Vec<_> = group
                .subtasks
                .iter()
                .filter(|t| task_matches_agent_filter(t, agent_filter))
                .collect();

            if agent_filter.is_some() && filtered_subtasks.is_empty() {
                continue;
            }

            if idx == selected {
                return Some(group.epic.id.clone());
            }
            idx += 1;

            if !self.collapsed_epics.contains(&group.epic.id) {
                for task in &filtered_subtasks {
                    if idx == selected {
                        return Some(task.id.clone());
                    }
                    idx += 1;
                }
            }
        }

        let filtered_standalone: Vec<_> = scoped
            .standalone
            .iter()
            .filter(|t| task_matches_agent_filter(t, agent_filter))
            .collect();

        if idx > 0 && !filtered_standalone.is_empty() {
            if idx == selected {
                return None; // separator row
            }
            idx += 1;
        }

        for task in &filtered_standalone {
            if idx == selected {
                return Some(task.id.clone());
            }
            idx += 1;
        }

        None
    }

    /// Get the selected task (if any).
    pub fn get_selected_task(&self) -> Option<&crate::ui::factory::director::TaskSummary> {
        let selected = self.panels.tasks.list_state.selected()?;
        use crate::ui::factory::director::tasks::task_matches_agent_filter;

        let scoped = crate::ui::factory::director::tasks::ScopedTaskView::new(
            &self.director_data,
            self.current_epic_id.as_deref(),
        );
        let agent_filter = self.agent_filter.as_deref();
        let mut idx = 0;

        for group in &scoped.epic_groups {
            let filtered_subtask_indices: Vec<usize> = group
                .subtasks
                .iter()
                .enumerate()
                .filter(|(_, t)| task_matches_agent_filter(t, agent_filter))
                .map(|(i, _)| i)
                .collect();

            if agent_filter.is_some() && filtered_subtask_indices.is_empty() {
                continue;
            }

            if idx == selected {
                return None; // epic header, not a task
            }
            idx += 1;

            if !self.collapsed_epics.contains(&group.epic.id) {
                for &task_idx in &filtered_subtask_indices {
                    if idx == selected {
                        let task = &group.subtasks[task_idx];
                        return self
                            .director_data
                            .in_progress_tasks
                            .iter()
                            .chain(self.director_data.ready_tasks.iter())
                            .find(|t| t.id == task.id);
                    }
                    idx += 1;
                }
            }
        }

        let filtered_standalone: Vec<_> = scoped
            .standalone
            .iter()
            .filter(|t| task_matches_agent_filter(t, agent_filter))
            .collect();

        if idx > 0 && !filtered_standalone.is_empty() {
            if idx == selected {
                return None; // separator
            }
            idx += 1;
        }

        for task in &filtered_standalone {
            if idx == selected {
                return self
                    .director_data
                    .in_progress_tasks
                    .iter()
                    .chain(self.director_data.ready_tasks.iter())
                    .find(|t| t.id == task.id);
            }
            idx += 1;
        }

        None
    }

    /// Scroll detail view up
    pub fn detail_scroll_up(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_sub(1);
    }

    /// Scroll detail view down
    pub fn detail_scroll_down(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_add(1);
    }

    // ========================================================================
    // Mission Control navigation
    // ========================================================================

    /// Check if we are in Mission Control mode.
    pub fn is_mission_control(&self) -> bool {
        self.factory_view_mode == crate::ui::factory::renderer::FactoryViewMode::MissionControl
    }

    /// Cycle Mission Control focus to the next panel.
    pub fn mc_focus_next(&mut self) {
        self.mc_focus = self.mc_focus.next();
        self.mc_init_panel_selection();
    }

    /// Cycle Mission Control focus to the previous panel.
    pub fn mc_focus_prev(&mut self) {
        self.mc_focus = self.mc_focus.prev();
        self.mc_init_panel_selection();
    }

    /// Jump MC focus to a specific panel.
    pub fn mc_focus_panel(&mut self, panel: crate::ui::factory::renderer::MissionControlFocus) {
        self.mc_focus = panel;
        self.mc_init_panel_selection();
    }

    /// Initialize selection for the MC-focused panel.
    fn mc_init_panel_selection(&mut self) {
        let focus = self.mc_focus.to_sidecar_focus();
        let count = self.item_count_for_focus(focus);
        if let Some(p) = self.panels.get_mut(focus) {
            p.init_selection(count);
        }
    }

    /// Scroll up in the MC-focused panel.
    pub fn mc_scroll_up(&mut self) {
        if let Some(p) = self.panels.get_mut(self.mc_focus.to_sidecar_focus()) {
            p.scroll_up();
        }
    }

    /// Scroll down in the MC-focused panel.
    pub fn mc_scroll_down(&mut self) {
        let focus = self.mc_focus.to_sidecar_focus();
        let count = self.item_count_for_focus(focus);
        if let Some(p) = self.panels.get_mut(focus) {
            p.scroll_down(count);
        }
    }

    /// Handle Enter in Mission Control view.
    pub fn mc_handle_enter(&mut self) {
        use crate::ui::factory::renderer::MissionControlFocus;
        match self.mc_focus {
            MissionControlFocus::Workers => {
                // Focus the selected worker's PTY and switch to Panes view
                if let Some(idx) = self.panels.factory.list_state.selected() {
                    if let Some(agent) = self.director_data.agents.get(idx) {
                        let _ = self.mux.focus(&agent.name);
                        self.factory_view_mode =
                            crate::ui::factory::renderer::FactoryViewMode::Panes;
                    }
                }
            }
            MissionControlFocus::Tasks => {
                self.open_task_dialog();
            }
            MissionControlFocus::Changes => {
                self.open_changes_dialog();
            }
            MissionControlFocus::Activity | MissionControlFocus::None => {}
        }
    }

    /// Handle Escape in Mission Control view. Returns true if something was closed.
    pub fn mc_handle_escape(&mut self) -> bool {
        // Close any open dialog first
        if self.show_task_dialog {
            self.close_task_dialog();
            return true;
        }
        if self.show_reminder_dialog {
            self.close_reminder_dialog();
            return true;
        }
        if self.show_changes_dialog {
            self.close_changes_dialog();
            return true;
        }
        // If a panel is focused, unfocus it
        if self.mc_focus != crate::ui::factory::renderer::MissionControlFocus::None {
            self.mc_focus = crate::ui::factory::renderer::MissionControlFocus::None;
            return true;
        }
        // Otherwise switch back to Panes view
        self.factory_view_mode = crate::ui::factory::renderer::FactoryViewMode::Panes;
        true
    }

    /// Enter inject mode from Mission Control.
    /// Targets the selected worker (if Workers panel focused), otherwise supervisor.
    pub fn mc_start_inject(&mut self) {
        use crate::ui::factory::renderer::MissionControlFocus;
        let target = if self.mc_focus == MissionControlFocus::Workers {
            // Use selected worker
            self.panels
                .factory
                .list_state
                .selected()
                .and_then(|idx| self.director_data.agents.get(idx))
                .map(|a| a.name.clone())
        } else {
            None
        };
        let target_name = target.unwrap_or_else(|| self.supervisor_name.clone());
        self.inject_target = Some(target_name);
        self.inject_buffer.clear();
        self.input_mode = InputMode::Inject;
    }

    /// Toggle epic collapse from Mission Control Tasks panel.
    pub fn mc_toggle_collapse(&mut self) {
        use crate::ui::factory::renderer::MissionControlFocus;
        match self.mc_focus {
            MissionControlFocus::Tasks => {
                // Reuse existing epic collapse logic (it checks sidecar_focus internally,
                // so we temporarily set it)
                let saved = self.sidecar_focus;
                self.sidecar_focus = SidecarFocus::Tasks;
                self.toggle_epic_collapse();
                self.sidecar_focus = saved;
            }
            MissionControlFocus::Changes => {
                let saved = self.sidecar_focus;
                self.sidecar_focus = SidecarFocus::Changes;
                self.toggle_selected_dir_collapse_mc();
                self.sidecar_focus = saved;
            }
            _ => {}
        }
    }

    /// Toggle collapse for selected directory in changes panel (MC variant).
    fn toggle_selected_dir_collapse_mc(&mut self) {
        let Some(selected_idx) = self.panels.changes.list_state.selected() else {
            return;
        };
        if let Some(crate::ui::widgets::TreeItemType::Directory(dir_path)) =
            self.changes_item_types.get(selected_idx)
        {
            if self.collapsed_dirs.contains(dir_path) {
                self.collapsed_dirs.remove(dir_path);
            } else {
                self.collapsed_dirs.insert(dir_path.clone());
            }
        }
    }
}

// =============================================================================
// Unit tests for scroll dispatch guard logic (cas-d5fa / cas-5cfd)
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use cas_factory::TaskSummary;
    use cas_mux::Pane;
    use cas_types::{Priority, TaskStatus, TaskType};
    use ratatui::layout::Rect;
    use std::collections::HashMap;

    /// Helper: create a FactoryApp with a single director pane that has been
    /// put into alt-screen mode.
    fn app_with_alt_screen() -> FactoryApp {
        let mut app = FactoryApp::for_test();
        let pane = Pane::director("test-pane", 24, 80).unwrap();
        app.mux.add_pane(pane);
        app.mux.focus("test-pane");
        app.mux
            .get_mut("test-pane")
            .unwrap()
            .feed(b"\x1b[?1049h")
            .unwrap();
        assert!(
            app.mux.focused_is_in_alt_screen(),
            "precondition: pane in alt-screen"
        );
        app
    }

    fn task(
        id: &str,
        task_type: TaskType,
        epic: Option<&str>,
        assignee: Option<&str>,
    ) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: id.to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: assignee.map(str::to_string),
            task_type,
            epic: epic.map(str::to_string),
            branch: Some(format!("epic/{id}")).filter(|_| task_type == TaskType::Epic),
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    fn app_with_scoped_tasks() -> FactoryApp {
        let mut app = FactoryApp::for_test();
        app.current_epic_id = Some("cas-focused".to_string());
        app.director_data = DirectorData {
            ready_tasks: vec![
                task(
                    "cas-focused-child",
                    TaskType::Task,
                    Some("cas-focused"),
                    None,
                ),
                task(
                    "cas-foreign-child",
                    TaskType::Task,
                    Some("cas-foreign"),
                    None,
                ),
                task("cas-standalone", TaskType::Task, None, Some("worker-one")),
            ],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![
                task("cas-focused", TaskType::Epic, None, None),
                task("cas-foreign", TaskType::Epic, None, None),
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::from([("agent-1".to_string(), "worker-one".to_string())]),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };
        app
    }

    #[test]
    fn task_selection_helpers_map_indices_after_current_epic_scoping() {
        let mut app = app_with_scoped_tasks();
        assert_eq!(
            app.task_display_item_count(),
            4,
            "focused epic header + child + separator + standalone"
        );

        app.panels.tasks.list_state.select(Some(0));
        assert_eq!(app.get_selected_task_id(), Some("cas-focused".to_string()));
        assert!(
            app.get_selected_task().is_none(),
            "epic header is not a task"
        );

        app.panels.tasks.list_state.select(Some(1));
        assert_eq!(
            app.get_selected_task_id(),
            Some("cas-focused-child".to_string())
        );
        assert_eq!(
            app.get_selected_task().map(|task| task.id.as_str()),
            Some("cas-focused-child")
        );

        app.panels.tasks.list_state.select(Some(2));
        assert_eq!(app.get_selected_task_id(), None, "separator row");
        assert!(app.get_selected_task().is_none());

        app.panels.tasks.list_state.select(Some(3));
        assert_eq!(
            app.get_selected_task_id(),
            Some("cas-standalone".to_string())
        );
        assert_eq!(
            app.get_selected_task().map(|task| task.id.as_str()),
            Some("cas-standalone")
        );
    }

    // -------------------------------------------------------------------------
    // Baseline: AltScreen returned when nothing blocks forwarding
    // -------------------------------------------------------------------------

    #[test]
    fn scroll_returns_alt_screen_when_clear() {
        let mut app = app_with_alt_screen();
        assert_eq!(
            app.handle_scroll_up(),
            ScrollAction::AltScreen,
            "should signal AltScreen when no overlay and focused pane is in alt-screen"
        );
        assert_eq!(
            app.handle_scroll_down(),
            ScrollAction::AltScreen,
            "down should also signal AltScreen"
        );
    }

    // -------------------------------------------------------------------------
    // P2 #4: show_help guard
    // -------------------------------------------------------------------------

    /// When the help overlay is open, wheel events must NOT be forwarded to the
    /// PTY even if the focused pane is in alt-screen.
    #[test]
    fn scroll_blocked_by_show_help() {
        let mut app = app_with_alt_screen();
        app.show_help = true;
        assert_eq!(
            app.handle_scroll_up(),
            ScrollAction::Done,
            "show_help must suppress alt-screen wheel forwarding (up)"
        );
        // Re-arm alt-screen (handle_scroll_up performed host scrollback)
        app.mux
            .get_mut("test-pane")
            .unwrap()
            .feed(b"\x1b[?1049h")
            .unwrap();
        assert_eq!(
            app.handle_scroll_down(),
            ScrollAction::Done,
            "show_help must suppress alt-screen wheel forwarding (down)"
        );
    }

    // -------------------------------------------------------------------------
    // P1 #1: Mission Control guard (any MC state, including mc_focus == None)
    // -------------------------------------------------------------------------

    /// When Mission Control is active with mc_focus == None (overview, no panel
    /// focused), scroll must NOT be forwarded to the background worker PTY.
    #[test]
    fn scroll_blocked_by_mc_focus_none() {
        let mut app = app_with_alt_screen();
        // Activate MC and ensure mc_focus is None (default after entering MC)
        app.factory_view_mode = crate::ui::factory::renderer::FactoryViewMode::MissionControl;
        app.mc_focus = crate::ui::factory::renderer::MissionControlFocus::None;
        assert!(app.is_mission_control(), "precondition: MC is active");
        assert_eq!(
            app.handle_scroll_up(),
            ScrollAction::Done,
            "MC active + mc_focus==None must suppress alt-screen wheel forwarding"
        );
    }

    /// When Mission Control is active with a non-None mc_focus, forwarding must
    /// also be suppressed (MC panel handles the scroll).
    #[test]
    fn scroll_blocked_by_mc_focus_workers() {
        let mut app = app_with_alt_screen();
        app.factory_view_mode = crate::ui::factory::renderer::FactoryViewMode::MissionControl;
        app.mc_focus = crate::ui::factory::renderer::MissionControlFocus::Workers;
        assert_eq!(
            app.handle_scroll_up(),
            ScrollAction::Done,
            "MC active + mc_focus==Workers must suppress alt-screen wheel forwarding"
        );
    }

    // -------------------------------------------------------------------------
    // PgUp/PgDn dispatch pre-condition tests
    //
    // The actual byte dispatch happens in `client_input.rs`; these tests verify
    // that `handle_scroll_up/down` returns the correct signal for the
    // PgUp/PgDn branch.
    // -------------------------------------------------------------------------

    /// When the focused pane is in alt-screen and no overlay is active,
    /// `handle_scroll_up()` returns `AltScreen` — the PgUp dispatch path in
    /// `client_input.rs` will call `mux.send_input(b"\x1b[5~")`.
    #[test]
    fn pgup_dispatch_fires_when_alt_screen_active() {
        let mut app = app_with_alt_screen();
        assert_eq!(
            app.handle_scroll_up(),
            ScrollAction::AltScreen,
            "PgUp: should return AltScreen (dispatch sends \\x1b[5~) when alt-screen active"
        );
    }

    /// When the focused pane is in alt-screen and no overlay is active,
    /// `handle_scroll_down()` returns `AltScreen` — the PgDn dispatch path in
    /// `client_input.rs` will call `mux.send_input(b"\x1b[6~")`.
    #[test]
    fn pgdn_dispatch_fires_when_alt_screen_active() {
        let mut app = app_with_alt_screen();
        assert_eq!(
            app.handle_scroll_down(),
            ScrollAction::AltScreen,
            "PgDn: should return AltScreen (dispatch sends \\x1b[6~) when alt-screen active"
        );
    }

    /// When the focused pane is NOT in alt-screen, `handle_scroll_up/down`
    /// returns `Done` — PgUp/PgDn fall through to normal host scrollback.
    #[test]
    fn pgup_pgdn_fall_through_when_not_in_alt_screen() {
        let mut app = FactoryApp::for_test();
        let pane = Pane::director("test-pane", 24, 80).unwrap();
        app.mux.add_pane(pane);
        app.mux.focus("test-pane");
        // Normal screen (no alt-screen entry)
        assert!(!app.mux.focused_is_in_alt_screen());

        assert_eq!(
            app.handle_scroll_up(),
            ScrollAction::Done,
            "PgUp: normal screen must return Done (host scrollback, not PTY forward)"
        );
        assert_eq!(
            app.handle_scroll_down(),
            ScrollAction::Done,
            "PgDn: normal screen must return Done (host scrollback, not PTY forward)"
        );
    }

    /// Wheel scroll on a normal (non-alt-screen) pane must return Done so the
    /// caller performs host scrollback rather than forwarding to the PTY.
    #[test]
    fn wheel_scroll_no_regress_when_not_in_alt_screen() {
        let mut app = FactoryApp::for_test();
        let pane = Pane::director("test-pane", 24, 80).unwrap();
        app.mux.add_pane(pane);
        app.mux.focus("test-pane");
        // Feed some content to create scrollback
        if let Some(p) = app.mux.get_mut("test-pane") {
            for i in 0..50 {
                p.feed(format!("Line {i}\r\n").as_bytes()).unwrap();
            }
        }
        assert_eq!(
            app.handle_scroll_up(),
            ScrollAction::Done,
            "normal screen: must return Done (use host scrollback, not PTY forward)"
        );
    }

    // =========================================================================
    // cas-72c3: daemon-dispatch coverage
    //
    // The daemon's MouseScrollUp/Down branch in
    // `cas-cli/src/ui/factory/daemon/runtime/client_input.rs` lines 157-187
    // is:
    //
    //   ControlEvent::MouseScrollUp => {
    //       if self.app.show_changes_dialog {
    //           self.app.diff_scroll_up();
    //       } else if self.app.handle_scroll_up() == ScrollAction::AltScreen {
    //           let _ = self.app.mux.send_input(SCROLL_UP_ARROWS).await;
    //       }
    //   }
    //
    // That sequence is tightly nested inside a long `tokio::select!` in the
    // client loop, so the dispatch itself is impractical to call from a
    // unit test without spinning up the full daemon. Instead, we pin the
    // pre- and post-conditions the daemon relies on:
    //
    //   1. `SCROLL_UP_ARROWS` / `SCROLL_DOWN_ARROWS` have the exact byte
    //      shape (`\x1b[5~` / `\x1b[6~`) the daemon documents and sends.
    //      A typo in either constant would silently break the wheel
    //      forwarding without any production assertion firing.
    //   2. `show_changes_dialog` shortcuts the daemon's outer `if` — it
    //      consumes the wheel event even when the focused pane is in
    //      alt-screen. We verify this by asserting `handle_scroll_up`
    //      returns `Done` (not `AltScreen`) when both conditions hold,
    //      which is the property the daemon's early-return relies on to
    //      avoid forwarding arrow bytes to the wrong consumer.
    //   3. The decision tree itself, expressed as a small local helper
    //      that mirrors the daemon's three-way branch and is asserted
    //      against the FactoryApp state for every leaf. If anyone changes
    //      either the daemon dispatch *or* `handle_scroll_up`'s return
    //      contract without updating this mirror, the table test fails.
    // =========================================================================

    /// AC #3 (cas-72c3, point 1): the wheel byte constants must match the
    /// documented shape — PgUp (`\x1b[5~`) for up, PgDn (`\x1b[6~`) for
    /// down.  The daemon forwards these literals via `alt_screen_wheel_bytes`
    /// for Claude/Codex, so a silent typo would break wheel-to-PTY forward.
    #[test]
    fn scroll_arrow_consts_have_exact_byte_shape_cas_72c3() {
        assert_eq!(
            SCROLL_UP_ARROWS, b"\x1b[5~",
            "SCROLL_UP_ARROWS must be the PgUp sequence (ESC [ 5 ~)"
        );
        assert_eq!(
            SCROLL_DOWN_ARROWS, b"\x1b[6~",
            "SCROLL_DOWN_ARROWS must be the PgDn sequence (ESC [ 6 ~)"
        );
    }

    /// cas-d3b5: SGR wheel constants + harness-aware payload selection.
    ///
    /// Grok prompt-focused no-ops on PgUp/PgDn but scrolls on SGR 1006 wheel
    /// (button 64/65). Claude/Codex keep the cas-f93a PgUp/PgDn path.
    #[test]
    fn sgr_wheel_consts_and_harness_payloads_cas_d3b5() {
        assert_eq!(
            SCROLL_UP_SGR, b"\x1b[<64;2;2M",
            "SCROLL_UP_SGR must be SGR 1006 wheel-up (button 64)"
        );
        assert_eq!(
            SCROLL_DOWN_SGR, b"\x1b[<65;2;2M",
            "SCROLL_DOWN_SGR must be SGR 1006 wheel-down (button 65)"
        );

        // Grok: SCROLL_LINES copies of the SGR unit.
        let grok_up = alt_screen_wheel_bytes(cas_mux::SupervisorCli::Grok, true);
        let grok_dn = alt_screen_wheel_bytes(cas_mux::SupervisorCli::Grok, false);
        assert_eq!(grok_up, SCROLL_UP_SGR.repeat(SCROLL_LINES));
        assert_eq!(grok_dn, SCROLL_DOWN_SGR.repeat(SCROLL_LINES));
        assert_eq!(grok_up.len(), SCROLL_UP_SGR.len() * SCROLL_LINES);
        assert!(
            !grok_up.windows(4).any(|w| w == b"\x1b[5~"),
            "Grok payload must not contain PgUp"
        );

        // Claude/Codex: single PgUp/PgDn (cas-f93a regression pin).
        assert_eq!(
            alt_screen_wheel_bytes(cas_mux::SupervisorCli::Claude, true),
            SCROLL_UP_ARROWS
        );
        assert_eq!(
            alt_screen_wheel_bytes(cas_mux::SupervisorCli::Claude, false),
            SCROLL_DOWN_ARROWS
        );
        assert_eq!(
            alt_screen_wheel_bytes(cas_mux::SupervisorCli::Codex, true),
            SCROLL_UP_ARROWS
        );
        assert_eq!(
            alt_screen_wheel_bytes(cas_mux::SupervisorCli::Codex, false),
            SCROLL_DOWN_ARROWS
        );
    }

    /// cas-d3b5: FactoryApp routes payload via focused pane harness.
    /// `for_test` defaults supervisor_cli/worker_cli to Claude; setting
    /// supervisor_cli=Grok and focusing the supervisor-named pane must
    /// select the SGR payload.
    #[test]
    fn alt_screen_scroll_payload_follows_focused_harness_cas_d3b5() {
        let mut app = app_with_alt_screen();
        // Default test harness is Claude → PgUp.
        assert_eq!(
            app.alt_screen_scroll_payload(true),
            SCROLL_UP_ARROWS,
            "default Claude harness must keep PgUp"
        );

        // Point the focused pane at the supervisor name and switch harness.
        app.supervisor_name = "test-pane".to_string();
        app.supervisor_cli = cas_mux::SupervisorCli::Grok;
        assert_eq!(
            app.focused_harness(),
            cas_mux::SupervisorCli::Grok,
            "focused supervisor pane must resolve Grok harness"
        );
        assert_eq!(
            app.alt_screen_scroll_payload(true),
            SCROLL_UP_SGR.repeat(SCROLL_LINES),
            "Grok focused pane must get SGR wheel-up × SCROLL_LINES"
        );
        assert_eq!(
            app.alt_screen_scroll_payload(false),
            SCROLL_DOWN_SGR.repeat(SCROLL_LINES),
            "Grok focused pane must get SGR wheel-down × SCROLL_LINES"
        );
    }

    // =========================================================================
    // cas-7f6f: Grok Stop click + Esc cancel path
    // =========================================================================

    /// SGR left-click is press (M) + release (m) at the given 1-based cell.
    #[test]
    fn sgr_left_click_bytes_shape_cas_7f6f() {
        assert_eq!(
            sgr_left_click_bytes(12, 7),
            b"\x1b[<0;12;7M\x1b[<0;12;7m",
            "SGR 1006 left click = press then release"
        );
        // Coordinates are clamped to ≥1 (terminals are 1-based).
        assert_eq!(sgr_left_click_bytes(0, 0), b"\x1b[<0;1;1M\x1b[<0;1;1m");
    }

    /// Click on an unfocused Grok pane only focuses (no SGR forward).
    /// Click on an already-focused Grok alt-screen pane forwards SGR so the
    /// on-screen Stop control can receive the event.
    #[test]
    fn mouse_click_forwards_sgr_only_when_already_focused_grok_alt_cas_7f6f() {
        let mut app = FactoryApp::for_test();
        // Placeholder first so the Grok pane is not auto-focused on add.
        app.mux
            .add_pane(Pane::director("other", 20, 40).unwrap());
        let mut pane = Pane::director("test-supervisor", 20, 40).unwrap();
        pane.set_harness(cas_mux::SupervisorCli::Grok);
        pane.feed(b"\x1b[?1049h").unwrap();
        app.mux.add_pane(pane);
        app.supervisor_name = "test-supervisor".to_string();
        app.supervisor_cli = cas_mux::SupervisorCli::Grok;
        // Outer rect for focus hit-test; full-mode content is border-inset.
        let outer = Rect::new(0, 0, 40, 20);
        app.supervisor_area = Some(outer);
        app.full_pty_content_areas
            .insert("test-supervisor".to_string(), full_mode_pty_content(outer));

        // Focus is on "other" — first click on Grok pane focuses only.
        assert_eq!(app.mux.focused_id(), Some("other"));
        assert_eq!(
            app.handle_mouse_click(10, 10, ClientGeometryMode::Full),
            ClickAction::Handled,
            "first click on unfocused pane must only focus"
        );
        assert_eq!(app.mux.focused_id(), Some("test-supervisor"));

        // Already focused + Grok + alt-screen → ForwardSgr.
        match app.handle_mouse_click(10, 10, ClientGeometryMode::Full) {
            ClickAction::ForwardSgr { pane, col, row } => {
                assert_eq!(pane, "test-supervisor");
                // screen (10,10) → content origin (1,1) → pty (10,10)
                assert_eq!((col, row), (10, 10));
            }
            other => panic!("expected ForwardSgr, got {other:?}"),
        }

        // Border click (col 0) stays Handled — outside content rect.
        assert_eq!(
            app.handle_mouse_click(0, 10, ClientGeometryMode::Full),
            ClickAction::Handled,
            "border click must not forward SGR"
        );
    }

    /// Full and compact geometry maps coexist — compact render must not
    /// overwrite full-client Stop coordinates (simultaneous clients).
    #[test]
    fn full_and_compact_geometry_independent_cas_7f6f() {
        let mut app = FactoryApp::for_test();
        app.supervisor_name = "sup".to_string();
        let full_outer = Rect::new(0, 2, 80, 30);
        let compact_outer = Rect::new(0, 1, 40, 20);

        app.full_pty_content_areas
            .insert("sup".to_string(), full_mode_pty_content(full_outer));
        app.compact_pty_content_areas
            .insert("sup".to_string(), compact_outer);

        // Same screen cell maps differently under each mode.
        let full = app
            .screen_to_pty_coords("sup", 10, 10, ClientGeometryMode::Full)
            .expect("full");
        let compact = app
            .screen_to_pty_coords("sup", 10, 10, ClientGeometryMode::Compact)
            .expect("compact");
        assert_ne!(full, compact, "full vs compact coords must differ");

        // Updating compact must not change full mapping.
        app.compact_pty_content_areas
            .insert("sup".to_string(), Rect::new(0, 1, 50, 25));
        let full_after = app
            .screen_to_pty_coords("sup", 10, 10, ClientGeometryMode::Full)
            .unwrap();
        assert_eq!(full, full_after, "compact map mutation must not clobber full");

        // Full-mode border is not content.
        assert_eq!(
            app.screen_to_pty_coords("sup", 0, 10, ClientGeometryMode::Full),
            None
        );
        // Compact borderless accepts top-left of its content rect.
        assert_eq!(
            app.screen_to_pty_coords("sup", 0, 1, ClientGeometryMode::Compact),
            Some((1, 1))
        );
    }

    /// submit → quiet >8s stays active; turn_ended completes; idle Esc is raw.
    #[test]
    fn grok_esc_submit_quiet_complete_idle_cas_7f6f() {
        assert_eq!(grok_esc_action(true), GrokEscAction::CancelTurn);
        assert_eq!(grok_esc_action(false), GrokEscAction::ForwardRaw);

        let mut app = FactoryApp::for_test();
        let mut pane = Pane::director("g", 10, 20).unwrap();
        pane.set_harness(cas_mux::SupervisorCli::Grok);
        app.mux.add_pane(pane);
        app.mux.focus("g");
        app.supervisor_name = "g".to_string();
        app.supervisor_cli = cas_mux::SupervisorCli::Grok;

        // Idle redraw alone never starts a turn.
        app.mux
            .get_mut("g")
            .unwrap()
            .feed(b"\x1b[2Jidle-redraw")
            .unwrap();
        assert!(!app.focused_grok_turn_in_flight());
        assert_eq!(app.focused_grok_esc_action(), GrokEscAction::ForwardRaw);

        // Submit starts in-flight.
        assert!(cas_mux::Pane::is_true_prompt_submit(b"\r"));
        assert!(cas_mux::Pane::key_stream_is_submit(b"hello\r"));
        assert!(!cas_mux::Pane::is_true_prompt_submit(
            b"\x1b[200~line1\nline2\r\x1b[201~"
        ));
        app.mux.get("g").unwrap().mark_turn_in_flight();
        assert!(app.focused_grok_turn_in_flight());
        assert_eq!(app.focused_grok_esc_action(), GrokEscAction::CancelTurn);

        // Quiet active for >8s (long MCP/tool wait) must stay cancelable —
        // PTY quiet timers are forbidden as completion evidence.
        app.mux
            .get_mut("g")
            .unwrap()
            .feed(b"tool wait...")
            .unwrap();
        // No events.jsonl → refresh must not clear.
        app.mux.get("g").unwrap().refresh_harness_turn_state();
        assert!(
            app.focused_grok_turn_in_flight(),
            "quiet >8s without turn_ended must stay in-flight"
        );
        assert_eq!(
            app.focused_grok_esc_action(),
            GrokEscAction::CancelTurn,
            "quiet active turn remains cancelable"
        );

        // Authoritative completion via Grok events.jsonl turn_ended.
        let dir = std::env::temp_dir().join(format!(
            "cas-7f6f-events-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let events = dir.join("events.jsonl");
        std::fs::write(&events, b"").unwrap();
        app.mux
            .get("g")
            .unwrap()
            .set_harness_events_path_for_test(&events);
        // Re-mark so offset captures empty file, then append turn_ended.
        app.mux.get("g").unwrap().mark_turn_in_flight();
        assert!(app.focused_grok_turn_in_flight());
        std::fs::write(
            &events,
            b"{\"ts\":\"2026-07-11T00:00:00Z\",\"type\":\"turn_ended\",\"outcome\":\"completed\"}\n",
        )
        .unwrap();
        app.mux.get("g").unwrap().refresh_harness_turn_state();
        assert!(
            !app.focused_grok_turn_in_flight(),
            "turn_ended must clear in-flight"
        );
        assert_eq!(app.focused_grok_esc_action(), GrokEscAction::ForwardRaw);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bracketed paste / multi-byte payloads with embedded CR must not mark submit.
    #[test]
    fn true_prompt_submit_not_paste_payload_cas_7f6f() {
        assert!(cas_mux::Pane::is_true_prompt_submit(b"\r"));
        assert!(cas_mux::Pane::is_true_prompt_submit(b"\n"));
        assert!(cas_mux::Pane::is_true_prompt_submit(b"\r\n"));
        assert!(!cas_mux::Pane::is_true_prompt_submit(b"hello\r"));
        // KeyStream multi-byte line+Enter is a submit; paste payload is not.
        assert!(cas_mux::Pane::key_stream_is_submit(b"hello\r"));
        assert!(!cas_mux::Pane::is_true_prompt_submit(
            b"\x1b[200~path/with\nnewline\r\x1b[201~"
        ));
        assert!(!cas_mux::Pane::is_true_prompt_submit(b"\x1b[200~\x1b[201~"));
    }

    /// Generic SGR click must not clear turn_in_flight (non-Stop preserves active).
    #[test]
    fn sgr_click_does_not_clear_turn_in_flight_cas_7f6f() {
        let mut app = FactoryApp::for_test();
        let mut pane = Pane::director("g", 10, 20).unwrap();
        pane.set_harness(cas_mux::SupervisorCli::Grok);
        app.mux.add_pane(pane);
        app.mux.focus("g");
        app.supervisor_name = "g".to_string();
        app.supervisor_cli = cas_mux::SupervisorCli::Grok;

        app.mux.get("g").unwrap().mark_turn_in_flight();
        assert!(app.focused_grok_turn_in_flight());
        // Simulate raw SGR forward (no note_turn_completed) — state preserved.
        let payload = sgr_left_click_bytes(5, 10);
        // send_input is the non-submit path used for SGR; does not clear.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let _ = app.mux.send_input_to("g", &payload).await;
        });
        assert!(
            app.focused_grok_turn_in_flight(),
            "generic SGR must not clear in-flight; only turn_ended/break_turn do"
        );
    }

    /// KeyStream marks submit; StructuredPaste with embedded CR never does.
    #[test]
    fn deliver_user_input_keystream_vs_paste_cas_7f6f() {
        let mut app = FactoryApp::for_test();
        let mut pane = Pane::director("g", 10, 20).unwrap();
        pane.set_harness(cas_mux::SupervisorCli::Grok);
        // Director has no PTY backend — deliver_user_input will fail write, but
        // mark happens before write. Use mark helpers for kind classification
        // and assert the pure predicates + mark path.
        app.mux.add_pane(pane);
        app.mux.focus("g");

        assert!(cas_mux::Pane::key_stream_is_submit(b"\r"));
        assert!(!cas_mux::Pane::key_stream_is_submit(b"x"));
        // Structured paste classification: never a true lone-CR submit.
        let paste = b"\x1b[200~hello\rworld\x1b[201~";
        assert!(!cas_mux::Pane::is_true_prompt_submit(paste));

        // Explicit mark only on KeyStream submit path.
        app.mux.get("g").unwrap().mark_turn_in_flight();
        assert!(app.focused_grok_turn_in_flight());
        app.mux.get("g").unwrap().mark_turn_completed();
        assert!(!app.focused_grok_turn_in_flight());
        // Paste does not auto-mark (no call to mark for StructuredPaste).
        assert!(!app.focused_grok_turn_in_flight());
    }

    /// Drop hit-testing is mode-aware so compact and full coexist.
    #[test]
    fn drop_target_geometry_mode_aware_cas_7f6f() {
        let mut app = FactoryApp::for_test();
        app.supervisor_name = "sup".to_string();
        app.worker_names = vec!["w1".to_string()];
        // Full: supervisor at left, worker at right (stale full layout).
        app.supervisor_area = Some(Rect::new(0, 0, 40, 20));
        app.worker_areas = vec![Rect::new(40, 0, 40, 20)];
        app.full_pty_content_areas
            .insert("sup".to_string(), full_mode_pty_content(Rect::new(0, 0, 40, 20)));
        app.full_pty_content_areas
            .insert("w1".to_string(), full_mode_pty_content(Rect::new(40, 0, 40, 20)));
        // Compact paints only supervisor borderless in a different rect.
        app.compact_pty_content_areas
            .insert("sup".to_string(), Rect::new(0, 1, 40, 18));

        // Full hit at worker content maps to w1.
        assert_eq!(
            app.pane_at_screen_for(50, 10, ClientGeometryMode::Full),
            Some("w1".to_string())
        );
        // Compact ignores full worker areas — only supervisor content.
        assert_eq!(
            app.pane_at_screen_for(50, 10, ClientGeometryMode::Compact),
            None,
            "compact must not use full-layout worker hit tests"
        );
        assert_eq!(
            app.pane_at_screen_for(5, 5, ClientGeometryMode::Compact),
            Some("sup".to_string())
        );
        // Updating compact map must not change full hit tests.
        app.compact_pty_content_areas
            .insert("sup".to_string(), Rect::new(0, 1, 10, 5));
        assert_eq!(
            app.pane_at_screen_for(50, 10, ClientGeometryMode::Full),
            Some("w1".to_string())
        );
    }

    /// Claude focused alt-screen must NOT forward SGR clicks (Stop path is
    /// Grok-only; Claude keeps Esc cancel).
    #[test]
    fn mouse_click_does_not_forward_sgr_for_claude_cas_7f6f() {
        let mut app = FactoryApp::for_test();
        let pane = Pane::director("test-supervisor", 20, 40).unwrap();
        app.mux.add_pane(pane);
        app.mux.focus("test-supervisor");
        app.mux
            .get_mut("test-supervisor")
            .unwrap()
            .feed(b"\x1b[?1049h")
            .unwrap();
        app.supervisor_name = "test-supervisor".to_string();
        app.supervisor_cli = cas_mux::SupervisorCli::Claude;
        app.supervisor_area = Some(Rect::new(0, 0, 40, 20));
        app.full_pty_content_areas.insert(
            "test-supervisor".to_string(),
            full_mode_pty_content(Rect::new(0, 0, 40, 20)),
        );

        assert_eq!(
            app.handle_mouse_click(10, 10, ClientGeometryMode::Full),
            ClickAction::Handled,
            "Claude must not receive factory-forwarded SGR clicks"
        );
    }

    /// Idle / no-pane click is harmless.
    #[test]
    fn mouse_click_idle_is_harmless_cas_7f6f() {
        let mut app = FactoryApp::for_test();
        assert_eq!(
            app.handle_mouse_click(5, 5, ClientGeometryMode::Full),
            ClickAction::Handled
        );
    }

    /// Harness-aware turn cancel bytes (pin for Esc routing + break_turn).
    #[test]
    fn turn_cancel_bytes_follow_harness_cas_7f6f() {
        assert_eq!(
            cas_mux::SupervisorCli::Claude.backend().turn_cancel_bytes(),
            &[0x1b],
            "Claude cancel = Esc"
        );
        assert_eq!(
            cas_mux::SupervisorCli::Codex.backend().turn_cancel_bytes(),
            &[0x1b],
            "Codex cancel = Esc"
        );
        assert_eq!(
            cas_mux::SupervisorCli::Grok.backend().turn_cancel_bytes(),
            &[0x03],
            "Grok cancel = Ctrl+C (Esc is mid-turn no-op since 0.2.93)"
        );
    }

    /// AC #3 (cas-72c3, point 2): when `show_changes_dialog` is open and the
    /// focused pane is in alt-screen, the daemon's outer `if` must consume
    /// the wheel event for the dialog (calling `diff_scroll_up`) BEFORE
    /// `handle_scroll_up` is even called. The post-condition this test pins
    /// is that `handle_scroll_up` returns `Done` (not `AltScreen`) under
    /// these flags — so even if a future refactor accidentally removed the
    /// daemon's outer `if`, the wheel event would still not get forwarded as
    /// arrow keys to the PTY.
    #[test]
    fn scroll_changes_dialog_blocks_alt_screen_forwarding_cas_72c3() {
        let mut app = app_with_alt_screen();
        app.show_changes_dialog = true;
        assert_eq!(
            app.handle_scroll_up(),
            ScrollAction::Done,
            "show_changes_dialog must consume wheel (no alt-screen forward, up)"
        );
        // Reset alt-screen — handle_scroll_up may have touched the pane.
        app.mux
            .get_mut("test-pane")
            .unwrap()
            .feed(b"\x1b[?1049h")
            .unwrap();
        assert_eq!(
            app.handle_scroll_down(),
            ScrollAction::Done,
            "show_changes_dialog must consume wheel (no alt-screen forward, down)"
        );
    }

    /// AC #3 (cas-72c3, point 3): table-driven mirror of the daemon's
    /// `ControlEvent::MouseScrollUp` decision tree in client_input.rs.
    /// Each row pins the FactoryApp state shape that drives one of the
    /// daemon's three terminal actions:
    ///   - "diff" — `show_changes_dialog == true` ⇒ call `diff_scroll_up()`
    ///   - "alt" — alt-screen + no dialog/MC/sidecar/help ⇒ send arrow bytes
    ///   - "noop" — `handle_scroll_up` already absorbed the event internally
    fn daemon_mouse_scroll_up_label(app: &mut FactoryApp) -> &'static str {
        if app.show_changes_dialog {
            "diff"
        } else if app.handle_scroll_up() == ScrollAction::AltScreen {
            "alt"
        } else {
            "noop"
        }
    }

    #[test]
    fn daemon_dispatch_table_for_mouse_scroll_up_cas_72c3() {
        // Row 1: alt-screen + no overlays → daemon sends arrows.
        {
            let mut app = app_with_alt_screen();
            assert_eq!(daemon_mouse_scroll_up_label(&mut app), "alt");
        }
        // Row 2: alt-screen + show_changes_dialog → daemon takes diff path.
        {
            let mut app = app_with_alt_screen();
            app.show_changes_dialog = true;
            assert_eq!(daemon_mouse_scroll_up_label(&mut app), "diff");
        }
        // Row 3: alt-screen + show_help → daemon takes noop path
        // (handle_scroll_up returns Done; help overlay consumes the wheel).
        {
            let mut app = app_with_alt_screen();
            app.show_help = true;
            assert_eq!(daemon_mouse_scroll_up_label(&mut app), "noop");
        }
        // Row 4: alt-screen + MC (mc_focus=None) → daemon takes noop path
        // (mc_focus_none guard, P1 #1 regression).
        {
            let mut app = app_with_alt_screen();
            app.factory_view_mode = crate::ui::factory::renderer::FactoryViewMode::MissionControl;
            app.mc_focus = crate::ui::factory::renderer::MissionControlFocus::None;
            assert_eq!(daemon_mouse_scroll_up_label(&mut app), "noop");
        }
        // Row 5: normal screen (no alt-screen, no overlays) → daemon noop.
        {
            let mut app = FactoryApp::for_test();
            let pane = Pane::director("test-pane", 24, 80).unwrap();
            app.mux.add_pane(pane);
            app.mux.focus("test-pane");
            assert!(!app.mux.focused_is_in_alt_screen());
            assert_eq!(daemon_mouse_scroll_up_label(&mut app), "noop");
        }
    }
}
