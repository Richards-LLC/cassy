use crate::ui::factory::app::imports::*;
use crate::ui::factory::buffer_backend::HyperlinkMap;
use std::sync::Arc;

const IDENTITY_HEADER_MIN_HEIGHT: u16 = 20;

/// Compact backend label shown beside a pane role.
///
/// Keep Claude's established shorthand (`cc`) so role tags stay narrow in
/// split-pane layouts while still making heterogeneous sessions legible.
pub(crate) fn backend_tag(cli: cas_mux::SupervisorCli) -> &'static str {
    match cli {
        cas_mux::SupervisorCli::Claude => "cc",
        cas_mux::SupervisorCli::Codex => "codex",
        cas_mux::SupervisorCli::Grok => "grok",
    }
}

pub(crate) fn role_tag(role: &str, cli: cas_mux::SupervisorCli) -> String {
    format!("[{role}:{}]", backend_tag(cli))
}

/// Content rect for a full-mode pane with `Borders::ALL` (1-cell border inset).
///
/// Matches `ratatui::widgets::Block::inner` for a default all-sides border so
/// Stop-click / PTY coordinates align with painted cells (cas-7f6f).
pub(crate) fn bordered_pty_content(outer: Rect) -> Rect {
    // Keep identical to `full_mode_pty_content` in sidecar_and_selection.
    crate::ui::factory::app::sidecar_and_selection::full_mode_pty_content(outer)
}

impl FactoryApp {
    pub(crate) fn full_pane_hyperlink_map(&self) -> HyperlinkMap {
        self.full_pane_hyperlinks.clone()
    }

    pub(crate) fn compact_pane_hyperlink_map(&self) -> HyperlinkMap {
        self.compact_pane_hyperlinks.clone()
    }

    fn clear_pane_hyperlinks(hyperlink_map: &HyperlinkMap) {
        if let Ok(mut hyperlinks) = hyperlink_map.lock() {
            hyperlinks.clear();
        }
    }

    fn prune_pane_hyperlinks(hyperlink_map: &HyperlinkMap, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        if let Ok(mut hyperlinks) = hyperlink_map.lock() {
            let x_end = area.x.saturating_add(area.width);
            let y_end = area.y.saturating_add(area.height);
            hyperlinks.retain(|(x, y), _| *x < area.x || *x >= x_end || *y < area.y || *y >= y_end);
        }
    }

    pub(crate) fn prune_full_pane_hyperlinks(&self, area: Rect) {
        Self::prune_pane_hyperlinks(&self.full_pane_hyperlinks, area);
    }

    fn record_pane_hyperlinks(
        &self,
        hyperlink_map: &HyperlinkMap,
        pane: &cas_mux::Pane,
        area: Rect,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let Ok(mut hyperlinks) = hyperlink_map.lock() else {
            return;
        };

        for row in 0..area.height {
            let row_links = pane.row_hyperlinks(row);
            for col in 0..area.width.min(row_links.len() as u16) {
                if let Some(uri) = row_links[col as usize].as_ref() {
                    hyperlinks.insert((area.x + col, area.y + row), Arc::from(uri.as_str()));
                }
            }
        }
    }

    fn sync_selected_worker_tab_with_focus(&mut self) {
        let Some(focused) = self.mux.focused() else {
            return;
        };
        if focused.kind() != &PaneKind::Worker {
            return;
        }
        if let Some(idx) = self
            .worker_names
            .iter()
            .position(|name| name == focused.id())
        {
            self.selected_worker_tab = idx;
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        Self::clear_pane_hyperlinks(&self.full_pane_hyperlinks);
        use crate::ui::factory::renderer::FactoryViewMode;
        match self.factory_view_mode {
            FactoryViewMode::Panes => self.render_panes_view(frame),
            FactoryViewMode::MissionControl => self.render_mission_control_view(frame),
        }
    }

    /// Render the standard Panes view (workers + supervisor + sidecar).
    fn render_panes_view(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.sync_selected_worker_tab_with_focus();

        // Calculate layout using all worker names (real + pending)
        let all_names = self.layout_worker_names();
        let header_rows = Self::identity_header_rows(area);
        let layout = FactoryLayout::calculate_from_names_with_header_rows(
            area,
            &all_names,
            self.tabbed_workers,
            self.sidecar_collapsed,
            self.layout_sizes,
            header_rows,
        );

        // Store layout areas for click detection
        self.worker_tab_bar_area = layout.worker_tab_bar;
        self.worker_content_area = layout.worker_content;
        self.worker_areas = layout.worker_areas.clone();
        self.supervisor_area = Some(layout.supervisor_area);
        self.sidecar_area = Some(layout.sidecar_area);
        // Full-mode panes always render with Borders::ALL — PTY content is the
        // bordered inner rect. Written only to full_pty_content_areas so a later
        // compact render does not overwrite full-client Stop geometry (cas-7f6f).
        self.full_pty_content_areas.clear();
        self.full_pty_content_areas.insert(
            self.supervisor_name.clone(),
            bordered_pty_content(layout.supervisor_area),
        );
        if layout.is_tabbed {
            if let Some(content) = layout.worker_content {
                if let Some(name) = all_names.get(self.selected_worker_tab) {
                    self.full_pty_content_areas
                        .insert(name.clone(), bordered_pty_content(content));
                }
            }
        } else {
            for (i, name) in all_names.iter().enumerate() {
                if let Some(area) = layout.worker_areas.get(i) {
                    self.full_pty_content_areas
                        .insert(name.clone(), bordered_pty_content(*area));
                }
            }
        }

        // Update pane grid if tabbed mode changed
        if self.is_tabbed != layout.is_tabbed {
            self.is_tabbed = layout.is_tabbed;
            self.pane_grid =
                PaneGrid::new(&self.worker_names, &self.supervisor_name, layout.is_tabbed);
        }

        self.render_identity_header(frame, layout.header_bar);

        // Render worker panes (stacked vertically)
        self.render_workers(frame, &layout);

        // Render supervisor pane
        self.render_supervisor(frame, &layout);

        // Render sidecar panels (Tasks, Agents, Changes, Activity)
        self.render_sidecar(frame, &layout);

        // Render status bar
        StatusBar::render(frame, layout.status_bar, self);
        self.render_error_banner(frame, layout.status_bar);

        self.render_overlays(frame);
    }

    pub(crate) fn identity_header_rows(area: Rect) -> u16 {
        u16::from(area.height >= IDENTITY_HEADER_MIN_HEIGHT)
    }

    fn render_identity_header(&self, frame: &mut Frame, area: Rect) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        use ratatui::style::Modifier;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::Paragraph;

        let styles = &self.theme.styles;
        let session_name = self.factory_session.as_deref().unwrap_or("factory");
        let worker_count = self.layout_worker_names().len();
        let worker_label = if worker_count == 1 {
            "1 worker".to_string()
        } else {
            format!("{worker_count} workers")
        };

        let mut spans = vec![Span::styled(
            session_name.to_string(),
            styles.text_primary.add_modifier(Modifier::BOLD),
        )];

        if let Some(epic_id) = self.current_epic_id.as_deref() {
            if let Some(epic) = self
                .director_data
                .epic_tasks
                .iter()
                .find(|task| task.id == epic_id)
            {
                spans.push(Span::styled(" · ", styles.text_muted));
                spans.push(Span::styled(
                    format!("EPIC {} {}", epic.id, truncate_chars(&epic.title, 28)),
                    styles.text_info,
                ));
            }

            if let Some(branch) = self.focused_epic_branch_status() {
                spans.push(Span::styled(" · ", styles.text_muted));
                spans.push(Span::styled(
                    crate::ui::factory::app::truncate_branch_middle(&branch.branch, 36),
                    styles.text_muted,
                ));
            }
        }

        // Session elapsed time is identity info, independent of epic focus.
        if let Some(created_at) = self.session_created_at {
            spans.push(Span::styled(" · ", styles.text_muted));
            spans.push(Span::styled(format_elapsed(created_at), styles.text_muted));
        }

        spans.push(Span::styled(" · ", styles.text_muted));
        spans.push(Span::styled(worker_label, styles.text_muted));

        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// Render the Mission Control dashboard view.
    ///
    /// Shows status strip, WORKERS panel and live TASKS, CHANGES, ACTIVITY columns.
    /// PTY panes keep running in the background (not rendered).
    fn render_mission_control_view(&mut self, frame: &mut Frame) {
        use crate::ui::factory::director::{mission_epic, mission_workers};
        use crate::ui::factory::layout::MissionControlLayout;
        use crate::ui::factory::renderer::MissionControlFocus;

        let area = frame.area();

        let worker_count = self.layout_worker_names().len();
        let mc = MissionControlLayout::calculate(area, worker_count);
        let focus = self.mc_focus;

        // Store MC panel areas for click detection
        self.mc_workers_area = mc.workers_area;
        self.mc_tasks_area = mc.tasks_area;
        self.mc_changes_area = mc.changes_area;
        self.mc_activity_area = mc.activity_area;

        // Status strip — compact epic progress + task counts + worker chips
        mission_epic::render_status_strip(
            frame,
            mc.status_strip,
            &self.director_data,
            &self.theme,
            self.current_epic_id.as_deref(),
        );

        // WORKERS panel — live content from DirectorData (focused border when selected)
        mission_workers::render_workers_panel_with_focus(
            frame,
            mc.workers_area,
            &self.director_data,
            &self.theme,
            focus == MissionControlFocus::Workers,
            if focus == MissionControlFocus::Workers {
                self.panels.factory.list_state.selected()
            } else {
                None
            },
        );

        // TASKS column — reuse the director tasks renderer
        let mission_control_scoped_tasks = crate::ui::factory::director::tasks::ScopedTaskView::new(
            &self.director_data,
            self.current_epic_id.as_deref(),
        );
        crate::ui::factory::director::tasks::render_with_focus(
            frame,
            mc.tasks_area,
            &self.director_data,
            &self.theme,
            focus == MissionControlFocus::Tasks,
            None,
            None,
            self.current_epic_id.as_deref(),
            &mission_control_scoped_tasks,
            false,
            &self.collapsed_epics,
            Some(&mut self.panels.tasks.list_state),
        );

        // CHANGES column — reuse the director changes renderer
        self.changes_item_types = crate::ui::factory::director::changes::render_with_focus(
            frame,
            mc.changes_area,
            &self.director_data,
            &self.theme,
            focus == MissionControlFocus::Changes,
            None,
            false,
            Some(&mut self.panels.changes.list_state),
            &self.collapsed_dirs,
        );

        // ACTIVITY column — reuse the director activity renderer
        let activity_backend_tags = self.activity_backend_tags();
        crate::ui::factory::director::activity::render_with_focus(
            frame,
            mc.activity_area,
            &self.director_data,
            &self.theme,
            focus == MissionControlFocus::Activity,
            None,
            false,
            &activity_backend_tags,
        );

        // Status bar
        StatusBar::render(frame, mc.status_bar, self);
        self.render_error_banner(frame, mc.status_bar);

        self.render_overlays(frame);
    }

    /// Render modal overlays (inject, help, dialogs).
    fn render_overlays(&mut self, frame: &mut Frame) {
        // Render inject dialog if in inject mode
        if matches!(self.input_mode, InputMode::Inject) {
            self.render_inject_dialog(frame);
        }

        // Render file changes dialog if visible
        if self.show_changes_dialog {
            self.render_changes_dialog(frame);
        }

        // Render task detail dialog if visible
        if self.show_task_dialog {
            self.render_task_dialog(frame);
        }

        // Render reminder detail dialog if visible
        if self.show_reminder_dialog {
            self.render_reminder_dialog(frame);
        }

        // Render terminal dialog if visible
        if self.show_terminal_dialog {
            self.render_terminal_dialog(frame);
        }

        // Render feedback dialog if visible
        if self.show_feedback_dialog {
            self.render_feedback_dialog(frame);
        }

        // Render help overlay if visible
        if self.show_help {
            self.render_help_overlay(frame);
        }
    }

    /// Render compact view for phone/narrow terminals.
    ///
    /// Layout: 1-line status bar at top, supervisor pane filling the rest.
    /// The supervisor pane is rendered without borders to maximize content area,
    /// since Claude Code has its own input UI.
    pub fn render_compact(&mut self, frame: &mut Frame) {
        use ratatui::layout::{Constraint, Layout};
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::Paragraph;

        let area = frame.area();
        Self::clear_pane_hyperlinks(&self.compact_pane_hyperlinks);

        // Split: 1-row status bar + rest for supervisor
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(3)])
            .split(area);

        let status_area = chunks[0];
        let supervisor_area = chunks[1];

        // Compact paints the supervisor borderless into the full content area
        // (see render body below). Update compact geometry only — do not touch
        // full_pty_content_areas (simultaneous full+compact clients, cas-7f6f).
        self.compact_pty_content_areas.clear();
        self.compact_pty_content_areas
            .insert(self.supervisor_name.clone(), supervisor_area);

        // === Compact status bar ===
        let palette = &self.theme().palette;
        let styles = &self.theme().styles;
        let mut spans = Vec::new();

        // CAS badge
        spans.push(Span::styled(
            " CAS ",
            Style::default()
                .fg(palette.chip_fg)
                .bg(palette.accent_dim)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));

        // Epic progress
        match self.epic_state() {
            EpicState::Idle => {
                spans.push(Span::styled("no epic", styles.text_muted));
            }
            EpicState::Active { epic_id, .. } | EpicState::Completing { epic_id, .. } => {
                // Get task counts from director data
                let data = self.director_data();
                let total =
                    data.epic_tasks.len() + data.ready_tasks.len() + data.in_progress_tasks.len();
                let done = data
                    .epic_tasks
                    .iter()
                    .filter(|t| t.status == cas_types::TaskStatus::Closed)
                    .count();

                let is_completing = matches!(self.epic_state(), EpicState::Completing { .. });

                spans.push(Span::styled(
                    epic_id.to_string(),
                    if is_completing {
                        styles.text_warning.add_modifier(Modifier::BOLD)
                    } else {
                        styles.text_success.add_modifier(Modifier::BOLD)
                    },
                ));
                spans.push(Span::raw(" "));

                // Progress bar
                let bar_width = 8usize;
                let filled = if total > 0 {
                    (done * bar_width) / total
                } else {
                    0
                };
                let empty = bar_width - filled;
                spans.push(Span::styled("\u{2588}".repeat(filled), styles.text_success));
                spans.push(Span::styled("\u{2591}".repeat(empty), styles.text_muted));
                spans.push(Span::styled(format!(" {done}/{total}"), styles.text_muted));
            }
        }

        // Worker status dots (right-aligned)
        let mut worker_spans = Vec::new();
        for name in self.worker_names() {
            let is_active = self
                .director_data()
                .agents
                .iter()
                .any(|a| a.name == *name && matches!(a.status, cas_types::AgentStatus::Active));

            let dot = if is_active { "\u{25cf}" } else { "\u{25cb}" };
            let short_name: String = name.chars().take(3).collect();
            worker_spans.push(Span::styled(
                format!("{dot}{short_name} "),
                if is_active {
                    styles.text_success
                } else {
                    styles.text_muted
                },
            ));
        }

        // Calculate padding between left and right
        let left_width: usize = spans.iter().map(|s| s.content.len()).sum();
        let right_width: usize = worker_spans.iter().map(|s| s.content.len()).sum();
        let padding = (status_area.width as usize).saturating_sub(left_width + right_width);

        spans.push(Span::raw(" ".repeat(padding)));
        spans.extend(worker_spans);

        let status_line = Paragraph::new(Line::from(spans)).style(styles.bg_elevated);
        frame.render_widget(status_line, status_area);

        // === Supervisor pane (no borders, full area) ===
        if let Some(pane) = self.mux.get(&self.supervisor_name) {
            let lines: Vec<Line> = (0..supervisor_area.height)
                .map(|row| pane.row_as_line(row).unwrap_or_default())
                .collect();

            let content = Paragraph::new(lines);
            frame.render_widget(content, supervisor_area);
            self.record_pane_hyperlinks(&self.compact_pane_hyperlinks, pane, supervisor_area);
        }
    }

    fn render_workers(&self, frame: &mut Frame, layout: &FactoryLayout) {
        let all_names = self.layout_worker_names();
        if layout.is_tabbed {
            // Tabbed mode: render tab bar + selected worker content
            if let Some(tab_bar_area) = layout.worker_tab_bar {
                self.render_worker_tab_bar(frame, tab_bar_area);
            }

            if let Some(content_area) = layout.worker_content {
                if let Some(name) = all_names.get(self.selected_worker_tab) {
                    if self.is_pending_worker(name) {
                        self.render_booting_pane(frame, content_area, name);
                    } else {
                        self.render_single_worker(frame, content_area, name);
                    }
                }
            }
        } else {
            // Side-by-side mode: render all workers in their own areas
            for (i, name) in all_names.iter().enumerate() {
                if let Some(worker_area) = layout.worker_areas.get(i) {
                    if self.is_pending_worker(name) {
                        self.render_booting_pane(frame, *worker_area, name);
                    } else {
                        self.render_single_worker(frame, *worker_area, name);
                    }
                }
            }
        }
    }

    /// Render a single worker pane
    fn render_single_worker(&self, frame: &mut Frame, area: Rect, name: &str) {
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

        if let Some(pane) = self.mux.get(name) {
            let palette = &self.theme().palette;
            let _agent_color = get_agent_color(name);
            let is_pane_select = self.input_mode.is_pane_select();
            let is_focused = pane.is_focused();

            // Get status indicator
            let status_icon = self.get_worker_status_icon(name);
            let title = format!(
                " {name}{status_icon} {} ",
                role_tag("worker", self.harness_for(name))
            );

            // Determine border style based on mode
            let (border_color, border_type) = if is_pane_select {
                if is_focused {
                    // Focused pane in PaneSelect: bright highlight with thick border
                    (palette.accent, BorderType::Thick)
                } else {
                    // Other panes in PaneSelect: visible but not focused
                    (palette.accent_dim, BorderType::Rounded)
                }
            } else {
                // Normal mode: muted border
                (palette.border_muted, BorderType::Rounded)
            };

            let block = Block::default()
                .title(title)
                .title_style(
                    Style::default()
                        .fg(border_color)
                        .add_modifier(Modifier::BOLD),
                )
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(Style::default().fg(border_color));

            let inner = block.inner(area);
            let lines: Vec<Line> = (0..inner.height)
                .map(|row| pane.row_as_line(row).unwrap_or_default())
                .collect();

            let content = Paragraph::new(lines).block(block);
            frame.render_widget(content, area);
            self.record_pane_hyperlinks(&self.full_pane_hyperlinks, pane, inner);

            // Show new-lines indicator when user has scrolled up
            let new_below = pane.new_lines_below();
            if pane.is_user_scrolled() && new_below > 0 {
                let label = format!(" ↓ {} new lines ", new_below);
                let label_width = label.len() as u16;
                let indicator_area = Rect {
                    x: inner.x + inner.width.saturating_sub(label_width),
                    y: inner.y + inner.height.saturating_sub(1),
                    width: label_width.min(inner.width),
                    height: 1,
                };
                let indicator = Paragraph::new(Line::from(Span::styled(
                    label,
                    Style::default()
                        .fg(ratatui::style::Color::Black)
                        .bg(ratatui::style::Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )));
                frame.render_widget(indicator, indicator_area);
            }
        }
    }

    /// Render the worker tab bar
    fn render_worker_tab_bar(&self, frame: &mut Frame, area: Rect) {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::Paragraph;

        let palette = &self.theme().palette;
        let all_names = self.layout_worker_names();

        // Braille spinner frames for pending workers
        const SPINNER_FRAMES: &[char] = &[
            '\u{2801}', '\u{2809}', '\u{2819}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
            '\u{2827}', '\u{2807}', '\u{280F}',
        ];

        // Tab bar background: darkest layer (bg_primary)
        let tab_bar_bg = palette.bg_primary;
        // Active tab: elevated surface to "pop" above the bar
        let active_tab_bg = palette.bg_elevated;
        // Inactive tab text: subdued
        let inactive_fg = palette.text_muted;
        // Border/separator color
        let border_color = palette.border_default;

        // -- Build the content line (middle row) --
        let mut content_spans: Vec<Span> = Vec::new();
        content_spans.push(Span::styled(" ", Style::default().bg(tab_bar_bg)));

        for (i, name) in all_names.iter().enumerate() {
            let is_selected = i == self.selected_worker_tab;
            let number = i + 1;
            let is_pending = self.is_pending_worker(name);

            let agent_color = if is_pending {
                Color::Rgb(255, 200, 80)
            } else {
                get_agent_color(name)
            };

            // Get status indicator from CAS data or spinner for pending
            let status_icon: String = if is_pending {
                let idx = (self
                    .pending_workers
                    .iter()
                    .find(|pw| pw.name == *name)
                    .map(|pw| pw.started_at.elapsed().as_millis() / 100)
                    .unwrap_or(0) as usize)
                    % SPINNER_FRAMES.len();
                format!(" {}", SPINNER_FRAMES[idx])
            } else {
                self.get_worker_status_icon(name).to_string()
            };

            let label = format!(
                " {number} {name}{status_icon} {} ",
                role_tag("worker", self.harness_for(name))
            );

            if is_selected {
                // Active tab: elevated bg, agent-colored text, bold
                content_spans.push(Span::styled(
                    label,
                    Style::default()
                        .fg(agent_color)
                        .bg(active_tab_bg)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                // Inactive tab: no bg (inherits bar bg), dimmed text
                content_spans.push(Span::styled(
                    label,
                    Style::default().fg(inactive_fg).bg(tab_bar_bg),
                ));
            }

            // Thin separator between tabs
            if i < all_names.len() - 1 {
                content_spans.push(Span::styled(
                    " ",
                    Style::default().fg(border_color).bg(tab_bar_bg),
                ));
            }
        }

        // Fill remaining width with bar background
        let used: usize = content_spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        let remaining = (area.width as usize).saturating_sub(used);
        if remaining > 0 {
            content_spans.push(Span::styled(
                " ".repeat(remaining),
                Style::default().bg(tab_bar_bg),
            ));
        }

        let content_line = Line::from(content_spans);

        let lines = if area.height >= 3 {
            // Top row: blank with bar bg
            let top_line = Line::from(Span::styled(
                " ".repeat(area.width as usize),
                Style::default().bg(tab_bar_bg),
            ));

            // Bottom row: border line with accent under active tab
            let mut bottom_spans: Vec<Span> = Vec::new();
            bottom_spans.push(Span::styled(
                "─",
                Style::default().fg(border_color).bg(tab_bar_bg),
            ));

            for (i, name) in all_names.iter().enumerate() {
                let is_selected = i == self.selected_worker_tab;
                let is_pending = self.is_pending_worker(name);

                let agent_color = if is_pending {
                    Color::Rgb(255, 200, 80)
                } else {
                    get_agent_color(name)
                };

                let status_icon: String = if is_pending {
                    let idx = (self
                        .pending_workers
                        .iter()
                        .find(|pw| pw.name == *name)
                        .map(|pw| pw.started_at.elapsed().as_millis() / 100)
                        .unwrap_or(0) as usize)
                        % SPINNER_FRAMES.len();
                    format!(" {}", SPINNER_FRAMES[idx])
                } else {
                    self.get_worker_status_icon(name).to_string()
                };
                let tab_label = format!(
                    " {} {}{} {} ",
                    i + 1,
                    name,
                    status_icon,
                    role_tag("worker", self.harness_for(name))
                );
                let tab_width = tab_label.chars().count();

                if is_selected {
                    // Active tab gets a colored underline accent
                    bottom_spans.push(Span::styled(
                        "▀".repeat(tab_width),
                        Style::default().fg(agent_color).bg(tab_bar_bg),
                    ));
                } else {
                    // Inactive tabs get a thin border line
                    bottom_spans.push(Span::styled(
                        "─".repeat(tab_width),
                        Style::default().fg(border_color).bg(tab_bar_bg),
                    ));
                }

                // Separator width
                if i < all_names.len() - 1 {
                    bottom_spans.push(Span::styled(
                        "─",
                        Style::default().fg(border_color).bg(tab_bar_bg),
                    ));
                }
            }

            // Fill remaining bottom with border
            let bottom_used: usize = bottom_spans.iter().map(|s| s.content.chars().count()).sum();
            let bottom_remaining = (area.width as usize).saturating_sub(bottom_used);
            if bottom_remaining > 0 {
                bottom_spans.push(Span::styled(
                    "─".repeat(bottom_remaining),
                    Style::default().fg(border_color).bg(tab_bar_bg),
                ));
            }

            let bottom_line = Line::from(bottom_spans);
            vec![top_line, content_line, bottom_line]
        } else {
            vec![content_line]
        };

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, area);
    }

    /// Get a status icon for a worker based on their CAS task state
    pub(crate) fn get_worker_status_icon(&self, worker_name: &str) -> &'static str {
        // Check if worker has an in-progress task
        for task in &self.director_data.in_progress_tasks {
            if let Some(assignee) = &task.assignee {
                if assignee == worker_name {
                    return " ●"; // Working indicator
                }
            }
        }
        "" // No status indicator
    }

    /// Render a booting pane placeholder for a pending worker
    fn render_booting_pane(&self, frame: &mut Frame, area: Rect, name: &str) {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

        let pending = self.pending_workers.iter().find(|pw| pw.name == name);
        let elapsed = pending
            .map(|pw| pw.started_at.elapsed())
            .unwrap_or_default();
        let is_isolate = pending.map(|pw| pw.isolate).unwrap_or(false);
        let elapsed_ms = elapsed.as_millis() as usize;

        // Colors matching boot screen
        let purple = Color::Rgb(180, 130, 255);
        let green = Color::Rgb(80, 250, 120);
        let orange = Color::Rgb(255, 200, 80);
        let gray = Color::Rgb(120, 120, 130);

        // Animated braille spinner for title
        const SPINNER: &[char] = &[
            '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
            '\u{2827}', '\u{2807}', '\u{280F}',
        ];
        let spinner_idx = (elapsed_ms / 80) % SPINNER.len();
        let spinner_char = SPINNER[spinner_idx];

        let title = format!(" {spinner_char} {name} [booting] ");

        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(purple).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(purple));

        let inner = block.inner(area);
        let mut lines: Vec<Line> = Vec::new();

        // Boot steps with status indicators
        let steps: Vec<(&str, u64)> = if is_isolate {
            vec![
                ("Resolving name", 0),
                ("Creating worktree", 500),
                ("Initializing branch", 2000),
                ("Spawning agent", 4000),
            ]
        } else {
            vec![("Resolving name", 0), ("Spawning agent", 500)]
        };

        // Blank line at top
        lines.push(Line::default());

        let elapsed_secs_ms = elapsed.as_millis() as u64;
        for (i, (step_name, start_ms)) in steps.iter().enumerate() {
            let is_last = i == steps.len() - 1;
            let next_start = steps.get(i + 1).map(|(_, ms)| *ms).unwrap_or(u64::MAX);

            let (icon, icon_color) = if elapsed_secs_ms >= next_start {
                // Completed
                ("\u{2714}", green) // checkmark
            } else if elapsed_secs_ms >= *start_ms {
                // In progress - animated spinner
                let step_spinner_idx = ((elapsed_secs_ms - start_ms) / 80) as usize % SPINNER.len();
                let c = SPINNER[step_spinner_idx];
                // We need to convert char to &str; use a match on a few cases
                let s: &str = match c {
                    '\u{280B}' => "\u{280B}",
                    '\u{2819}' => "\u{2819}",
                    '\u{2839}' => "\u{2839}",
                    '\u{2838}' => "\u{2838}",
                    '\u{283C}' => "\u{283C}",
                    '\u{2834}' => "\u{2834}",
                    '\u{2826}' => "\u{2826}",
                    '\u{2827}' => "\u{2827}",
                    '\u{2807}' => "\u{2807}",
                    '\u{280F}' => "\u{280F}",
                    _ => "\u{280B}",
                };
                (s, orange)
            } else {
                // Not started
                ("\u{25CB}", gray) // circle
            };

            let text_color = if elapsed_secs_ms >= *start_ms {
                if elapsed_secs_ms >= next_start {
                    green
                } else {
                    Color::Rgb(220, 220, 230)
                }
            } else {
                gray
            };

            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(icon.to_string(), Style::default().fg(icon_color)),
                Span::raw(" "),
                Span::styled(step_name.to_string(), Style::default().fg(text_color)),
            ]));

            // Add blank line between steps (if space allows)
            if !is_last && inner.height > 8 {
                lines.push(Line::default());
            }
        }

        // Progress bar
        lines.push(Line::default());
        let bar_width = (inner.width as usize).saturating_sub(4).min(30);
        if bar_width > 0 {
            // Progress based on elapsed time (cap at ~6 seconds for visual)
            let progress = (elapsed_secs_ms as f64 / 6000.0).min(0.95);
            let filled = (progress * bar_width as f64) as usize;
            let empty = bar_width - filled;

            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("\u{2588}".repeat(filled), Style::default().fg(purple)),
                Span::styled("\u{2591}".repeat(empty), Style::default().fg(gray)),
            ]));
        }

        // Pad remaining lines
        while lines.len() < inner.height as usize {
            lines.push(Line::default());
        }
        lines.truncate(inner.height as usize);

        let content = Paragraph::new(lines).block(block);
        frame.render_widget(content, area);
    }

    fn render_supervisor(&self, frame: &mut Frame, layout: &FactoryLayout) {
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

        if let Some(pane) = self.mux.get(&self.supervisor_name) {
            let palette = &self.theme().palette;
            // Supervisor is only truly focused if mux says so AND sidecar is not focused
            let is_focused = pane.is_focused() && !self.sidecar_is_focused();
            let _agent_color = get_agent_color(&self.supervisor_name);
            let is_pane_select = self.input_mode.is_pane_select();

            // Determine border style based on mode
            let (border_color, border_type) = if is_pane_select {
                if is_focused {
                    // Focused pane in PaneSelect: bright highlight with thick border
                    (palette.accent, BorderType::Thick)
                } else {
                    // Other panes in PaneSelect: visible but not focused
                    (palette.accent_dim, BorderType::Rounded)
                }
            } else if is_focused {
                // Normal mode, focused: use theme border color
                (palette.border_focused, BorderType::Rounded)
            } else {
                // Normal mode, not focused: dimmed
                (palette.border_muted, BorderType::Rounded)
            };

            let title = format!(
                " {} {} ",
                self.supervisor_name,
                role_tag("supervisor", self.supervisor_cli)
            );
            let block = Block::default()
                .title(title)
                .title_style(
                    Style::default()
                        .fg(border_color)
                        .add_modifier(Modifier::BOLD),
                )
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(Style::default().fg(border_color));

            let inner = block.inner(layout.supervisor_area);
            let lines: Vec<Line> = (0..inner.height)
                .map(|row| pane.row_as_line(row).unwrap_or_default())
                .collect();

            let content = Paragraph::new(lines).block(block);
            frame.render_widget(content, layout.supervisor_area);
            self.record_pane_hyperlinks(&self.full_pane_hyperlinks, pane, inner);

            // Show new-lines indicator when user has scrolled up
            let new_below = pane.new_lines_below();
            if pane.is_user_scrolled() && new_below > 0 {
                let label = format!(" ↓ {} new lines ", new_below);
                let label_width = label.len() as u16;
                let indicator_area = Rect {
                    x: inner.x + inner.width.saturating_sub(label_width),
                    y: inner.y + inner.height.saturating_sub(1),
                    width: label_width.min(inner.width),
                    height: 1,
                };
                let indicator = Paragraph::new(Line::from(Span::styled(
                    label,
                    Style::default()
                        .fg(ratatui::style::Color::Black)
                        .bg(ratatui::style::Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )));
                frame.render_widget(indicator, indicator_area);
            }
        }
    }

    fn render_sidecar(&mut self, frame: &mut Frame, layout: &FactoryLayout) {
        match &self.view_mode {
            ViewMode::Overview => {
                let focused_epic_branch_status = self
                    .focused_epic_branch_status()
                    .map(|status| (status.branch.clone(), status.ahead, status.behind));
                let mut state = SidecarState {
                    focus: self.sidecar_focus,
                    tasks_state: &mut self.panels.tasks.list_state,
                    agents_state: &mut self.panels.factory.list_state,
                    reminders_state: &mut self.panels.reminders.list_state,
                    changes_state: &mut self.panels.changes.list_state,
                    activity_state: &mut self.panels.activity.list_state,
                    agent_filter: self.agent_filter.as_deref(),
                    focused_epic_id: self.current_epic_id.as_deref(),
                    focused_epic_branch_status: focused_epic_branch_status.as_ref().map(|status| {
                        crate::ui::factory::director::EpicBranchStatus {
                            branch: status.0.as_str(),
                            ahead: status.1,
                            behind: status.2,
                        }
                    }),
                    factory_collapsed: self.panels.factory.collapsed,
                    tasks_collapsed: self.panels.tasks.collapsed,
                    reminders_collapsed: self.panels.reminders.collapsed,
                    changes_collapsed: self.panels.changes.collapsed,
                    activity_collapsed: self.panels.activity.collapsed,
                    collapsed_epics: &self.collapsed_epics,
                    collapsed_dirs: &self.collapsed_dirs,
                    changes_item_types: &mut self.changes_item_types,
                };
                let activity_backend_tags = self.activity_backend_tags();
                let areas = render_with_state(
                    frame,
                    layout.sidecar_area,
                    &self.director_data,
                    &self.theme,
                    self.current_epic_id.as_deref(),
                    &self.supervisor_name,
                    &activity_backend_tags,
                    Some(&mut state),
                );
                // Store panel areas for click detection
                self.panel_areas = areas;
            }
            ViewMode::TaskDetail(task_id) => {
                let task_id = task_id.clone();
                self.render_task_detail(frame, layout.sidecar_area, &task_id);
            }
            ViewMode::ActivityLog => {
                self.render_activity_log(frame, layout.sidecar_area);
            }
            ViewMode::FileDiff(_, file_path) => {
                let file_path = file_path.clone();
                self.render_file_diff(frame, layout.sidecar_area, &file_path);
            }
        }
    }

    fn activity_backend_tags(&self) -> HashMap<String, &'static str> {
        self.director_data
            .agent_id_to_name
            .iter()
            .map(|(agent_id, name)| (agent_id.clone(), backend_tag(self.harness_for(name))))
            .collect()
    }
}

#[cfg(test)]
mod hyperlink_tests {
    use super::FactoryApp;
    use crate::ui::factory::buffer_backend::BufferBackend;
    use cas_mux::Pane;
    use ratatui::backend::Backend;
    use ratatui::buffer::Cell;
    use ratatui::layout::Rect;
    use std::sync::Arc;

    #[test]
    fn pane_hyperlinks_are_recorded_at_final_screen_offsets_for_two_panes() {
        let app = FactoryApp::for_test();
        let mut left = Pane::director("left", 1, 20).unwrap();
        let mut right = Pane::director("right", 1, 20).unwrap();
        left.feed(b"\x1b]8;;https://left.example\x1b\\left\x1b]8;;\x1b\\")
            .unwrap();
        right
            .feed(b"\x1b]8;;https://right.example\x1b\\right\x1b]8;;\x1b\\")
            .unwrap();

        let map = app.full_pane_hyperlink_map();
        FactoryApp::clear_pane_hyperlinks(&map);
        app.record_pane_hyperlinks(
            &map,
            &left,
            Rect {
                x: 2,
                y: 3,
                width: 20,
                height: 1,
            },
        );
        app.record_pane_hyperlinks(
            &map,
            &right,
            Rect {
                x: 30,
                y: 3,
                width: 20,
                height: 1,
            },
        );

        let hyperlinks = map.lock().unwrap();
        assert_eq!(
            hyperlinks.get(&(2, 3)).map(AsRef::as_ref),
            Some("https://left.example")
        );
        assert_eq!(
            hyperlinks.get(&(30, 3)).map(AsRef::as_ref),
            Some("https://right.example")
        );
        assert_eq!(hyperlinks.get(&(0, 0)), None);
    }

    #[test]
    fn pane_hyperlinks_record_row_offsets_and_clip_to_area_width() {
        let app = FactoryApp::for_test();
        let mut pane = Pane::director("pane", 3, 20).unwrap();
        pane.feed(b"plain\r\n\x1b]8;;https://row.example\x1b\\linked\x1b]8;;\x1b\\")
            .unwrap();

        let map = app.full_pane_hyperlink_map();
        FactoryApp::clear_pane_hyperlinks(&map);
        app.record_pane_hyperlinks(
            &map,
            &pane,
            Rect {
                x: 5,
                y: 7,
                width: 3,
                height: 3,
            },
        );

        let hyperlinks = map.lock().unwrap();
        assert_eq!(
            hyperlinks.get(&(5, 8)).map(AsRef::as_ref),
            Some("https://row.example")
        );
        assert_eq!(
            hyperlinks.get(&(7, 8)).map(AsRef::as_ref),
            Some("https://row.example")
        );
        assert_eq!(hyperlinks.get(&(8, 8)), None);
        assert_eq!(hyperlinks.get(&(5, 7)), None);
    }

    #[test]
    fn overlay_pruning_removes_links_from_covered_cells_before_backend_emit() {
        let app = FactoryApp::for_test();
        let map = app.full_pane_hyperlink_map();
        {
            let mut hyperlinks = map.lock().unwrap();
            hyperlinks.insert((3, 2), Arc::from("https://covered.example"));
            hyperlinks.insert((8, 2), Arc::from("https://visible.example"));
        }

        app.prune_full_pane_hyperlinks(Rect {
            x: 2,
            y: 1,
            width: 4,
            height: 3,
        });

        let cells = [Cell::new("x"), Cell::new("y")];
        let mut backend = BufferBackend::with_hyperlinks(12, 4, map);
        backend
            .draw(vec![(3, 2, &cells[0]), (8, 2, &cells[1])].into_iter())
            .unwrap();

        let output = String::from_utf8(backend.take_buffer()).unwrap();
        assert!(!output.contains("https://covered.example"));
        assert!(output.contains("https://visible.example"));
    }

    #[test]
    fn full_and_compact_hyperlink_maps_are_independent() {
        let app = FactoryApp::for_test();
        let full = app.full_pane_hyperlink_map();
        let compact = app.compact_pane_hyperlink_map();

        full.lock()
            .unwrap()
            .insert((0, 0), Arc::from("https://full.example"));
        compact
            .lock()
            .unwrap()
            .insert((0, 0), Arc::from("https://compact.example"));

        FactoryApp::clear_pane_hyperlinks(&full);

        assert!(full.lock().unwrap().is_empty());
        assert_eq!(
            compact.lock().unwrap().get(&(0, 0)).map(AsRef::as_ref),
            Some("https://compact.example")
        );
    }
}

/// cas-eb7f (review finding, cas-ebc1 final): char-count-counterpart of
/// `crate::ui::widgets::utils::truncate` (byte-length based). Kept as a
/// separate function — callers here pass a `max_chars` *character* budget
/// (the identity header's epic-title slot), not a byte length — but unified
/// on `truncate()`'s "..." ellipsis convention/threshold so titles don't
/// visually truncate differently panel-to-panel depending on which helper
/// rendered them.
fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        // cas-6185c: the ellipsis indicator itself must never exceed the
        // caller's budget. A bare "..." (3 chars) for max_chars=0/1/2 used
        // to overflow by 1-3 chars — clamp it to whatever fits, degrading
        // to a partial ellipsis ("..", ".") or nothing at max_chars=0.
        return "...".chars().take(max_chars).collect();
    }
    value
        .chars()
        .take(max_chars - 3)
        .chain("...".chars())
        .collect()
}

fn format_elapsed(created_at: chrono::DateTime<chrono::Utc>) -> String {
    let elapsed_minutes = chrono::Utc::now()
        .signed_duration_since(created_at)
        .num_minutes()
        .max(0);
    let hours = elapsed_minutes / 60;
    let minutes = elapsed_minutes % 60;
    format!("{hours:02}:{minutes:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_factory::TaskSummary;
    use cas_types::{Priority, TaskStatus, TaskType};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn epic_task(id: &str, title: &str, branch: &str) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: title.to_string(),
            status: TaskStatus::InProgress,
            priority: Priority::MEDIUM,
            assignee: None,
            task_type: TaskType::Epic,
            epic: None,
            branch: Some(branch.to_string()),
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn identity_header_renders_session_epic_branch_elapsed_and_worker_count() {
        let mut app = FactoryApp::for_test();
        app.factory_session = Some("session-alpha".to_string());
        app.session_created_at = Some(chrono::Utc::now() - chrono::TimeDelta::minutes(125));
        app.current_epic_id = Some("cas-epic".to_string());
        app.worker_names = vec!["worker-one".to_string(), "worker-two".to_string()];
        app.director_data.epic_tasks = vec![epic_task(
            "cas-epic",
            "Visual Information Overhaul",
            "epic/factory-tui-overhaul",
        )];
        app.branch_visibility.insert_epic_ahead_behind(
            "cas-epic",
            "epic/factory-tui-overhaul",
            2,
            0,
        );

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("session-alpha"));
        assert!(text.contains("EPIC cas-epic Visual Information Overhaul"));
        assert!(text.contains("epic/factory-tui-overhaul"));
        assert!(text.contains("02:05"));
        assert!(text.contains("2 workers"));
    }

    #[test]
    fn identity_header_hides_below_height_threshold() {
        let mut app = FactoryApp::for_test();
        app.factory_session = Some("session-alpha".to_string());
        app.worker_names = vec!["worker-one".to_string()];

        let backend = TestBackend::new(100, 19);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();

        let text = buffer_text(&terminal);
        assert!(!text.contains("session-alpha"));
    }

    #[test]
    fn identity_header_degrades_without_focused_epic() {
        let mut app = FactoryApp::for_test();
        app.factory_session = Some("session-beta".to_string());
        app.worker_names = vec!["worker-one".to_string(), "worker-two".to_string()];
        // Elapsed clock is session identity — it must render with NO epic focused
        // (regression: it was nested inside the epic-focus block).
        app.session_created_at = Some(chrono::Utc::now() - chrono::Duration::minutes(90));

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("session-beta"));
        assert!(text.contains("2 workers"));
        assert!(!text.contains("EPIC "));
        assert!(
            text.contains("01:30"),
            "elapsed session time must render without a focused epic"
        );
    }

    /// cas-eb7f (review finding, cas-ebc1 final): the only existing test that
    /// renders an epic title never crosses the truncation threshold (27
    /// chars against max_chars=28), so the char-counting/ellipsis-chaining
    /// branch of `truncate_chars` was entirely unverified. Given this repo
    /// shipped a hotfix for a char-boundary panic in a sibling truncation
    /// helper (1ba84f1), an untested truncation function that runs on
    /// arbitrary (including multi-byte) task titles is a real gap.
    #[test]
    fn truncate_chars_below_and_at_threshold_returns_unchanged() {
        assert_eq!(truncate_chars("short", 28), "short");
        let exact = "exactly28characterslong!!!!!";
        assert_eq!(
            exact.chars().count(),
            28,
            "fixture must be exactly 28 chars"
        );
        assert_eq!(truncate_chars(exact, 28), exact);
    }

    #[test]
    fn truncate_chars_over_threshold_truncates_with_unified_ellipsis() {
        // max_chars=28, kept prefix is (max_chars - 3) chars + "..."
        // (cas-eb7f: unified with crate::ui::widgets::utils::truncate's
        // "..." convention, not the old single "…" glyph).
        let title = "This Title Is Definitely Long";
        assert!(title.chars().count() > 28, "fixture must exceed max_chars");
        let truncated = truncate_chars(title, 28);
        assert_eq!(truncated.chars().count(), 28);
        assert!(truncated.ends_with("..."));
        let expected_prefix: String = title.chars().take(28 - 3).collect();
        assert_eq!(truncated, format!("{expected_prefix}..."));
    }

    /// Multi-byte content must truncate on a char boundary, not a byte
    /// index — the exact failure mode of the sibling hotfix (1ba84f1) this
    /// finding referenced.
    #[test]
    fn truncate_chars_multibyte_content_does_not_panic_or_split_a_char() {
        let title = "日本語のタイトルがとても長い場合のテスト"; // multi-byte, > 10 chars
        let truncated = truncate_chars(title, 10);
        assert_eq!(truncated.chars().count(), 10);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn truncate_chars_max_chars_at_ellipsis_width_returns_bare_ellipsis() {
        assert_eq!(truncate_chars("a much longer title than this", 3), "...");
    }

    /// cas-6185c AC5: below the ellipsis width (0/1/2), the result must
    /// never exceed `max_chars` — a bare "..." (3 chars) used to be
    /// returned unconditionally, silently overflowing the caller's budget
    /// by up to 3 characters.
    #[test]
    fn truncate_chars_below_ellipsis_width_never_exceeds_max_chars() {
        let long = "a much longer title than this";

        let t2 = truncate_chars(long, 2);
        assert_eq!(t2.chars().count(), 2, "max_chars=2 result: {t2:?}");
        assert_eq!(t2, "..");

        let t1 = truncate_chars(long, 1);
        assert_eq!(t1.chars().count(), 1, "max_chars=1 result: {t1:?}");
        assert_eq!(t1, ".");

        let t0 = truncate_chars(long, 0);
        assert_eq!(t0.chars().count(), 0, "max_chars=0 result: {t0:?}");
        assert_eq!(t0, "");
    }

    #[test]
    fn role_tags_identify_every_supported_backend() {
        for (cli, backend) in [
            (cas_mux::SupervisorCli::Claude, "cc"),
            (cas_mux::SupervisorCli::Codex, "codex"),
            (cas_mux::SupervisorCli::Grok, "grok"),
        ] {
            assert_eq!(role_tag("worker", cli), format!("[worker:{backend}]"));
            assert_eq!(
                role_tag("supervisor", cli),
                format!("[supervisor:{backend}]")
            );
        }
    }
}
