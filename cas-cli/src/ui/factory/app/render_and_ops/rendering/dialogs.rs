use crate::ui::factory::app::imports::*;

impl FactoryApp {
    pub(crate) fn render_error_banner(&self, frame: &mut Frame, status_bar_area: Rect) {
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Clear, Paragraph};

        let Some(error) = &self.error_message else {
            return;
        };
        if status_bar_area.y == 0 {
            return;
        }

        let palette = &self.theme().palette;
        let styles = &self.theme().styles;
        let banner_area = Rect::new(
            status_bar_area.x,
            status_bar_area.y.saturating_sub(1),
            status_bar_area.width,
            1,
        );
        self.prune_full_pane_hyperlinks(banner_area);

        let prefix = " ERROR ";
        let dismiss = "  Ctrl+E dismiss ";
        let available = banner_area.width as usize;
        let fixed = prefix.len() + dismiss.len() + 1; // +1 spacing before dismiss
        let max_msg = available.saturating_sub(fixed).max(1);
        let truncated = if error.chars().count() > max_msg {
            let kept: String = error.chars().take(max_msg.saturating_sub(1)).collect();
            format!("{kept}…")
        } else {
            error.clone()
        };

        let line = Line::from(vec![
            Span::styled(
                prefix,
                Style::default()
                    .fg(palette.chip_fg)
                    .bg(palette.status_error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {truncated}"),
                Style::default()
                    .fg(palette.chip_fg)
                    .bg(palette.status_error),
            ),
            Span::styled(
                dismiss,
                Style::default()
                    .fg(palette.chip_fg)
                    .bg(palette.status_error)
                    .add_modifier(Modifier::DIM),
            ),
        ]);

        frame.render_widget(Clear, banner_area);
        frame.render_widget(Paragraph::new(line).style(styles.bg_primary), banner_area);
    }

    pub(crate) fn render_inject_dialog(&self, frame: &mut Frame) {
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

        let area = frame.area();
        let palette = &self.theme().palette;
        let styles = &self.theme().styles;
        if area.width < 24 || area.height < 7 {
            return;
        }

        // Center the dialog
        let dialog_width = area.width.saturating_sub(4).clamp(24, 60);
        let dialog_height = 7;
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);
        self.prune_full_pane_hyperlinks(dialog_area);

        // Clear area behind dialog
        frame.render_widget(Clear, dialog_area);

        let target_name = self.inject_target.as_deref().unwrap_or("unknown");
        let target_color = get_agent_color(target_name);

        // Calculate available width for input
        let input_width = dialog_width.saturating_sub(6) as usize;
        let display_buffer = if self.inject_buffer.len() > input_width {
            &self.inject_buffer[self.inject_buffer.len() - input_width..]
        } else {
            &self.inject_buffer
        };

        let text = vec![
            Line::from(vec![
                Span::styled(" To: ", styles.text_muted),
                Span::styled(
                    target_name,
                    Style::default()
                        .fg(target_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(" > ", styles.text_info),
                Span::styled(display_buffer, styles.text_primary),
                Span::styled("▌", styles.text_info),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(" ↵ ", styles.text_success),
                Span::styled("send  ", styles.text_muted),
                Span::styled("esc", styles.text_warning),
                Span::styled(" cancel", styles.text_muted),
            ]),
        ];

        let dialog = Paragraph::new(text).block(
            Block::default()
                .title(" Inject Prompt ")
                .title_style(
                    Style::default()
                        .fg(palette.status_info)
                        .add_modifier(Modifier::BOLD),
                )
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(styles.border_default)
                .style(styles.bg_elevated),
        );

        frame.render_widget(dialog, dialog_area);
    }

    pub(crate) fn render_changes_dialog(&mut self, frame: &mut Frame) {
        use cas_diffs::iter::DiffStyle;
        use cas_diffs::widget::DiffWidget;
        use ratatui::layout::Alignment;
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, StatefulWidget};

        let area = frame.area();
        if area.width < 30 || area.height < 12 {
            return;
        }

        // Copy theme values to avoid borrow conflicts with diff_view_state
        let styles_bg_secondary = self.theme().styles.bg_secondary;
        let styles_bg_elevated = self.theme().styles.bg_elevated;
        let text_muted = self.theme().styles.text_muted;
        let text_warning = self.theme().styles.text_warning;
        let text_primary = self.theme().styles.text_primary;
        let palette_status_success = self.theme().palette.status_success;
        let palette_status_error = self.theme().palette.status_error;
        let palette_status_warning = self.theme().palette.status_warning;

        // Large dialog for file changes - 85% of screen
        let dialog_width = (area.width * 85 / 100)
            .max(60)
            .min(area.width.saturating_sub(2));
        let dialog_height = (area.height * 85 / 100)
            .max(20)
            .min(area.height.saturating_sub(2));
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);
        self.prune_full_pane_hyperlinks(dialog_area);

        // Clear area behind dialog
        frame.render_widget(Clear, dialog_area);

        let (_source_path, file_path, source_name, agent_name) = match &self.changes_dialog_file {
            Some(info) => info.clone(),
            None => {
                let empty = Paragraph::new("No file selected")
                    .alignment(Alignment::Center)
                    .block(
                        Block::default()
                            .title(" File Changes ")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .style(styles_bg_elevated),
                    );
                frame.render_widget(empty, dialog_area);
                return;
            }
        };

        let display_name = agent_name.as_ref().unwrap_or(&source_name);
        let agent_color = get_agent_color(display_name);

        let style_label = match self.diff_display_style {
            DiffStyle::Split => "split",
            _ => "unified",
        };

        // Build title with stats
        let mut title_spans = vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                &file_path,
                Style::default()
                    .fg(agent_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" [{style_label}] "), text_muted),
        ];

        if let Some(ref diff) = self.diff_metadata {
            let mut additions = 0usize;
            let mut deletions = 0usize;
            for hunk in &diff.hunks {
                additions += hunk.addition_lines;
                deletions += hunk.deletion_lines;
            }
            title_spans.push(Span::styled(
                format!("+{additions}"),
                Style::default().fg(palette_status_success),
            ));
            title_spans.push(Span::raw("/"));
            title_spans.push(Span::styled(
                format!("-{deletions}"),
                Style::default().fg(palette_status_error),
            ));
        }

        if !self.diff_search_query.is_empty() {
            let match_info = if self.diff_search_matches.is_empty() {
                " (no matches)".to_string()
            } else {
                format!(
                    " ({}/{})",
                    self.diff_search_current + 1,
                    self.diff_search_matches.len()
                )
            };
            title_spans.push(Span::styled(
                match_info,
                Style::default().fg(palette_status_warning),
            ));
        }

        title_spans.push(Span::styled(" ", Style::default()));

        use ratatui::layout::{Constraint, Direction, Layout};

        let block = Block::default()
            .title(Line::from(title_spans))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(agent_color))
            .style(styles_bg_secondary);

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // Split inner area: diff content + footer
        let chunks = Layout::vertical([
            Constraint::Min(1),    // Diff content
            Constraint::Length(2), // Footer
        ])
        .split(inner);

        let diff_area_full = chunks[0];
        let footer_area = chunks[1];

        // Reserve space for search input if in search mode
        let (diff_area, search_area) = if self.diff_search_mode {
            let parts = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(diff_area_full);
            (parts[0], Some(parts[1]))
        } else {
            (diff_area_full, None)
        };

        // Render DiffWidget if metadata is available, else legacy fallback
        if let Some(ref diff) = self.diff_metadata {
            let diff = diff.clone();
            let mut widget = DiffWidget::new(&diff, self.diff_display_style)
                .show_file_header(false)
                .show_line_numbers(self.diff_show_line_numbers)
                .inline_diff_mode(self.diff_inline_mode)
                .expand_all(self.diff_expand_all)
                .highlighter(&self.syntax_highlighter);
            if !self.diff_expanded_hunks.is_empty() {
                widget = widget.expanded_hunks(&self.diff_expanded_hunks);
            }
            widget.render(diff_area, frame.buffer_mut(), &mut self.diff_view_state);
        } else {
            let msg = Paragraph::new("No diff data available");
            frame.render_widget(msg, diff_area);
        }

        // Render search input if in search mode
        if let Some(search_rect) = search_area {
            let cursor = "\u{2588}";
            let search_line = Line::from(vec![
                Span::styled("/", text_warning),
                Span::styled(&self.diff_search_query, text_primary),
                Span::styled(cursor, text_warning),
            ]);
            let search_widget = Paragraph::new(search_line).style(styles_bg_elevated);
            frame.render_widget(search_widget, search_rect);
        }

        // Render footer
        let sep_width = footer_area.width as usize;
        let total_lines =
            self.diff_metadata
                .as_ref()
                .map_or(0, |d| match self.diff_display_style {
                    DiffStyle::Split => d.split_line_count,
                    _ => d.unified_line_count,
                });
        let visible_height = diff_area.height as usize;
        let scroll_info = if total_lines > visible_height {
            let progress = if total_lines > visible_height {
                let start = self.diff_view_state.scroll_offset;
                ((start as f64 / (total_lines - visible_height) as f64) * 100.0) as u16
            } else {
                100
            };
            format!("  {progress}%")
        } else {
            String::new()
        };

        let inline_label = match self.diff_inline_mode {
            cas_diffs::LineDiffType::WordAlt => "word+",
            cas_diffs::LineDiffType::Word => "word",
            cas_diffs::LineDiffType::Char => "char",
            cas_diffs::LineDiffType::None => "off",
        };

        let footer = Paragraph::new(vec![
            Line::styled("\u{2500}".repeat(sep_width), text_muted),
            Line::from(vec![
                Span::styled(" Esc ", text_warning),
                Span::styled("close ", text_muted),
                Span::styled("j/k ", text_warning),
                Span::styled("scroll ", text_muted),
                Span::styled("]/[ ", text_warning),
                Span::styled("hunk ", text_muted),
                Span::styled("s ", text_warning),
                Span::styled("split ", text_muted),
                Span::styled("e ", text_warning),
                Span::styled(
                    if self.diff_expand_all {
                        "collapse "
                    } else {
                        "expand "
                    },
                    text_muted,
                ),
                Span::styled("d ", text_warning),
                Span::styled(format!("{inline_label} "), text_muted),
                Span::styled("l ", text_warning),
                Span::styled("lines ", text_muted),
                Span::styled("/ ", text_warning),
                Span::styled("search", text_muted),
                Span::styled(scroll_info, text_muted),
            ]),
        ]);
        frame.render_widget(footer, footer_area);
    }

    pub(crate) fn render_task_dialog(&mut self, frame: &mut Frame) {
        use crate::store::open_task_store;
        use ratatui::layout::Alignment;
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

        let area = frame.area();
        let palette = &self.theme().palette;
        let styles = &self.theme().styles;

        // Large centered dialog (80% width, 85% height) with minimum edge padding
        let dialog_width = ((area.width as f32 * 0.80) as u16)
            .min(120)
            .min(area.width.saturating_sub(4));
        let dialog_height = ((area.height as f32 * 0.85) as u16)
            .min(50)
            .min(area.height.saturating_sub(4));
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);
        self.prune_full_pane_hyperlinks(dialog_area);

        // Clear area behind dialog
        frame.render_widget(Clear, dialog_area);

        let task_id = match &self.task_dialog_id {
            Some(id) => id,
            None => {
                let empty = Paragraph::new("No task selected").block(
                    Block::default()
                        .title(" Task Detail ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .style(styles.bg_elevated),
                );
                frame.render_widget(empty, dialog_area);
                return;
            }
        };

        // Load full task from store (not just summary)
        let task = match open_task_store(&self.cas_dir) {
            Ok(store) => match store.get(task_id) {
                Ok(t) => t,
                Err(_) => {
                    let not_found = Paragraph::new(format!("Task {task_id} not found")).block(
                        Block::default()
                            .title(" Task Detail ")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .style(styles.bg_elevated),
                    );
                    frame.render_widget(not_found, dialog_area);
                    return;
                }
            },
            Err(_) => {
                let error = Paragraph::new("Failed to open task store").block(
                    Block::default()
                        .title(" Task Detail ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .style(styles.bg_elevated),
                );
                frame.render_widget(error, dialog_area);
                return;
            }
        };

        // Determine task color based on type
        let task_color = match task.task_type {
            cas_types::TaskType::Epic => palette.accent,
            cas_types::TaskType::Feature => palette.status_success,
            cas_types::TaskType::Bug => palette.status_error,
            _ => palette.status_info,
        };

        // Build content lines
        let mut lines: Vec<Line> = Vec::new();

        // Header with ID and Type
        lines.push(Line::from(vec![
            Span::styled("ID: ", styles.text_muted),
            Span::styled(&task.id, styles.entity_id),
            Span::raw("  "),
            Span::styled("Type: ", styles.text_muted),
            Span::styled(
                format!("{:?}", task.task_type),
                Style::default().fg(task_color),
            ),
            Span::raw("  "),
            Span::styled("Priority: ", styles.text_muted),
            Span::styled(
                format!("P{}", task.priority.0),
                Style::default().fg(match task.priority.0 {
                    0 => palette.priority_critical,
                    1 => palette.priority_high,
                    2 => palette.priority_medium,
                    3 => palette.priority_low,
                    _ => palette.priority_backlog,
                }),
            ),
        ]));
        lines.push(Line::from(""));

        // Title
        lines.push(Line::from(vec![
            Span::styled("Title: ", styles.text_muted),
            Span::styled(
                &task.title,
                styles.text_primary.add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        // Status and Assignee
        lines.push(Line::from(vec![
            Span::styled("Status: ", styles.text_muted),
            Span::styled(
                format!("{:?}", task.status),
                Style::default().fg(match task.status {
                    cas_types::TaskStatus::Open => palette.task_open,
                    cas_types::TaskStatus::InProgress => palette.task_in_progress,
                    cas_types::TaskStatus::Closed => palette.task_closed,
                    cas_types::TaskStatus::Cancelled => palette.task_closed,
                    cas_types::TaskStatus::Blocked => palette.task_blocked,
                    // cas-b51a: reuse warning color — task awaits supervisor review
                    cas_types::TaskStatus::PendingSupervisorReview => palette.task_blocked,
                    cas_types::TaskStatus::AwaitingMerge => palette.task_blocked,
                }),
            ),
            Span::raw("  "),
            Span::styled("Assignee: ", styles.text_muted),
            Span::styled(
                task.assignee.as_deref().unwrap_or("unassigned"),
                styles.text_success,
            ),
        ]));
        lines.push(Line::from(""));

        // Description section (markdown rendered)
        if !task.description.is_empty() {
            lines.push(Line::from(Span::styled(
                "═══ Description ═══",
                styles.text_warning.add_modifier(Modifier::BOLD),
            )));
            lines.extend(render_markdown(&task.description, self.theme()));
        }

        // Design section (markdown rendered)
        if !task.design.is_empty() {
            lines.push(Line::from(Span::styled(
                "═══ Design ═══",
                styles.text_info.add_modifier(Modifier::BOLD),
            )));
            lines.extend(render_markdown(&task.design, self.theme()));
        }

        // Acceptance Criteria section (markdown rendered)
        if !task.acceptance_criteria.is_empty() {
            lines.push(Line::from(Span::styled(
                "═══ Acceptance Criteria ═══",
                styles.text_success.add_modifier(Modifier::BOLD),
            )));
            lines.extend(render_markdown(&task.acceptance_criteria, self.theme()));
        }

        // Notes section (markdown rendered)
        if !task.notes.is_empty() {
            lines.push(Line::from(Span::styled(
                "═══ Notes ═══",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.extend(render_markdown(&task.notes, self.theme()));
        }

        // Labels
        if !task.labels.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Labels: ", styles.text_muted),
                Span::styled(task.labels.join(", "), styles.text_secondary),
            ]));
            lines.push(Line::from(""));
        }

        // Timestamps
        lines.push(Line::from(vec![
            Span::styled("Created: ", styles.text_muted),
            Span::styled(
                task.created_at.format("%Y-%m-%d %H:%M").to_string(),
                styles.text_secondary,
            ),
            Span::raw("  "),
            Span::styled("Updated: ", styles.text_muted),
            Span::styled(
                task.updated_at.format("%Y-%m-%d %H:%M").to_string(),
                styles.text_secondary,
            ),
        ]));

        use ratatui::layout::{Constraint, Layout};
        use ratatui::widgets::Padding;

        let total_content_lines = lines.len();

        // Render block border separately so we can split inner area
        let block = Block::default()
            .title(format!(" Task: {} ", task.title))
            .title_style(Style::default().fg(task_color).add_modifier(Modifier::BOLD))
            .title_alignment(Alignment::Left)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(task_color))
            .style(styles.bg_secondary)
            .padding(Padding::horizontal(1));

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // Split inner area into scrollable content + fixed footer
        let chunks = Layout::vertical([
            Constraint::Min(1),    // Content (fills remaining)
            Constraint::Length(2), // Footer (separator + hints)
        ])
        .split(inner);

        let content_area = chunks[0];
        let footer_area = chunks[1];

        // Render scrollable content with Paragraph::scroll
        let scroll = self.task_dialog_scroll;
        let content = Paragraph::new(lines)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(content, content_area);

        // Render fixed footer (always visible)
        let sep_width = footer_area.width as usize;
        let content_height = content_area.height as usize;
        let max_scroll = total_content_lines.saturating_sub(content_height) as u16;
        let footer = Paragraph::new(vec![
            Line::styled("─".repeat(sep_width), styles.text_muted),
            Line::from(vec![
                Span::styled(" Esc ", styles.text_warning),
                Span::styled("close  ", styles.text_muted),
                Span::styled("j/k ", styles.text_warning),
                Span::styled("scroll", styles.text_muted),
                if total_content_lines > content_height {
                    Span::styled(
                        format!(
                            "  [{}/{}]",
                            (scroll as usize + 1).min(total_content_lines),
                            total_content_lines
                        ),
                        styles.text_muted,
                    )
                } else {
                    Span::raw("")
                },
            ]),
        ]);
        frame.render_widget(footer, footer_area);
        self.task_dialog_max_scroll = max_scroll;
    }

    pub(crate) fn render_reminder_dialog(&mut self, frame: &mut Frame) {
        use cas_store::ReminderStatus;
        use ratatui::layout::Alignment;
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

        let area = frame.area();
        // Clone theme values upfront to avoid borrow conflicts when mutating self
        let palette = self.theme().palette.clone();
        let styles = self.theme().styles.clone();

        let dialog_width = ((area.width as f32 * 0.70) as u16)
            .min(100)
            .min(area.width.saturating_sub(4));
        let dialog_height = ((area.height as f32 * 0.60) as u16)
            .min(30)
            .min(area.height.saturating_sub(4));
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);
        self.prune_full_pane_hyperlinks(dialog_area);

        frame.render_widget(Clear, dialog_area);

        let reminder = match self.reminder_dialog_idx {
            Some(idx) => match self.director_data.reminders.get(idx) {
                Some(r) => r.clone(),
                None => {
                    let empty = Paragraph::new("Reminder not found").block(
                        Block::default()
                            .title(" Reminder Detail ")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .style(styles.bg_elevated),
                    );
                    frame.render_widget(empty, dialog_area);
                    return;
                }
            },
            None => return,
        };

        let status_str = match reminder.status {
            ReminderStatus::Pending => "Pending",
            ReminderStatus::Fired => "Fired",
            ReminderStatus::Cancelled => "Cancelled",
            ReminderStatus::Expired => "Expired",
        };
        let status_color = match reminder.status {
            ReminderStatus::Pending => palette.status_warning,
            ReminderStatus::Fired => palette.status_success,
            ReminderStatus::Cancelled => palette.text_muted,
            ReminderStatus::Expired => palette.text_muted,
        };

        let trigger_str = match reminder.trigger_type {
            cas_store::ReminderTriggerType::Time => "Time-based",
            cas_store::ReminderTriggerType::Event => "Event-based",
        };

        let mut lines: Vec<Line> = Vec::new();

        // Status and type
        lines.push(Line::from(vec![
            Span::styled("Status: ", styles.text_muted),
            Span::styled(
                status_str,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    "),
            Span::styled("Trigger: ", styles.text_muted),
            Span::styled(trigger_str, styles.text_info),
        ]));
        lines.push(Line::from(""));

        // Trigger details
        match reminder.trigger_type {
            cas_store::ReminderTriggerType::Time => {
                if let Some(at) = reminder.trigger_at {
                    lines.push(Line::from(vec![
                        Span::styled("Fire at: ", styles.text_muted),
                        Span::styled(
                            at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                            styles.text_primary,
                        ),
                    ]));
                }
            }
            cas_store::ReminderTriggerType::Event => {
                if let Some(ref event) = reminder.trigger_event {
                    lines.push(Line::from(vec![
                        Span::styled("Event: ", styles.text_muted),
                        Span::styled(event.clone(), styles.text_primary),
                    ]));
                }
                if let Some(ref filter) = reminder.trigger_filter {
                    lines.push(Line::from(vec![
                        Span::styled("Filter: ", styles.text_muted),
                        Span::styled(filter.to_string(), styles.text_primary),
                    ]));
                }
            }
        }
        lines.push(Line::from(""));

        // Ownership
        lines.push(Line::from(vec![
            Span::styled("Owner: ", styles.text_muted),
            Span::styled(&reminder.owner_id, styles.text_primary),
            Span::raw("    "),
            Span::styled("Target: ", styles.text_muted),
            Span::styled(&reminder.target_id, styles.text_primary),
        ]));

        // Timestamps
        lines.push(Line::from(vec![
            Span::styled("Created: ", styles.text_muted),
            Span::styled(
                reminder.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                styles.text_primary,
            ),
        ]));
        if let Some(fired_at) = reminder.fired_at {
            lines.push(Line::from(vec![
                Span::styled("Fired at: ", styles.text_muted),
                Span::styled(
                    fired_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                    styles.text_primary,
                ),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("TTL: ", styles.text_muted),
            Span::styled(format!("{}s", reminder.ttl_secs), styles.text_primary),
        ]));
        lines.push(Line::from(""));

        // Message (full, wrapped)
        lines.push(Line::from(vec![Span::styled(
            "Message:",
            styles.text_muted.add_modifier(Modifier::BOLD),
        )]));
        for msg_line in reminder.message.lines() {
            lines.push(Line::from(Span::styled(
                msg_line.to_string(),
                styles.text_primary,
            )));
        }

        // Fired event data
        if let Some(ref event_data) = reminder.fired_event {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "Fired Event:",
                styles.text_muted.add_modifier(Modifier::BOLD),
            )]));
            let formatted =
                serde_json::to_string_pretty(event_data).unwrap_or_else(|_| event_data.to_string());
            for line in formatted.lines() {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    styles.text_primary,
                )));
            }
        }

        let total_content_lines = lines.len();

        // Layout: block with inner split into content + footer
        let title = format!(" Reminder #{} ", reminder.id);
        let block = Block::default()
            .title(title)
            .title_style(
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.border_focused))
            .style(styles.bg_elevated);

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        if inner.height < 2 {
            return;
        }

        let content_height = inner.height.saturating_sub(2) as usize;
        let max_scroll = total_content_lines.saturating_sub(content_height) as u16;
        let scroll = self.reminder_dialog_scroll.min(max_scroll);
        self.reminder_dialog_scroll = scroll;

        let content_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(2),
        );
        let footer_area = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(2),
            inner.width,
            2,
        );

        let content = Paragraph::new(lines)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(content, content_area);

        // Footer
        let sep_width = footer_area.width as usize;
        let footer = Paragraph::new(vec![
            Line::styled("─".repeat(sep_width), styles.text_muted),
            Line::from(vec![
                Span::styled(" Esc ", styles.text_warning),
                Span::styled("close  ", styles.text_muted),
                Span::styled("↑↓ ", styles.text_warning),
                Span::styled("scroll", styles.text_muted),
                if total_content_lines > content_height {
                    Span::styled(
                        format!(
                            "  [{}/{}]",
                            (scroll as usize + 1).min(total_content_lines),
                            total_content_lines
                        ),
                        styles.text_muted,
                    )
                } else {
                    Span::raw("")
                },
            ]),
        ]);
        frame.render_widget(footer, footer_area);
    }

    pub(crate) fn render_feedback_dialog(&self, frame: &mut Frame) {
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

        let area = frame.area();
        let palette = &self.theme().palette;
        let styles = &self.theme().styles;

        // Centered dialog
        let dialog_width = 60.min(area.width.saturating_sub(4));
        let dialog_height = 12.min(area.height.saturating_sub(4));
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);
        self.prune_full_pane_hyperlinks(dialog_area);

        // Clear area behind dialog
        frame.render_widget(Clear, dialog_area);

        // Build category selector line
        let categories: Vec<Span> = crate::ui::factory::input::FeedbackCategory::all()
            .iter()
            .enumerate()
            .flat_map(|(i, cat)| {
                let is_selected = *cat == self.feedback_category;
                let style = if is_selected {
                    Style::default()
                        .fg(palette.chip_fg)
                        .bg(palette.status_warning)
                        .add_modifier(Modifier::BOLD)
                } else {
                    styles.text_muted
                };
                let mut spans = vec![Span::styled(format!(" {} ", cat.as_str()), style)];
                if i < crate::ui::factory::input::FeedbackCategory::all().len() - 1 {
                    spans.push(Span::raw("  "));
                }
                spans
            })
            .collect();

        // Show feedback text with cursor
        let display_text = if self.feedback_buffer.is_empty() {
            Span::styled("Type your feedback here...", styles.text_muted)
        } else {
            Span::styled(&self.feedback_buffer, styles.text_primary)
        };

        use ratatui::layout::{Constraint, Layout};
        use ratatui::widgets::Padding;

        // Render block separately for Layout split
        let block = Block::default()
            .title(" Send Feedback ")
            .title_style(
                Style::default()
                    .fg(palette.status_warning)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.status_warning))
            .style(styles.bg_elevated)
            .padding(Padding::horizontal(1));

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // Split inner area: content + footer
        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);

        let content_area = chunks[0];
        let footer_area = chunks[1];

        // Render content (without footer lines)
        let content_lines = vec![
            Line::from(""),
            Line::from(vec![Span::styled("Category: ", styles.text_muted)]),
            Line::from(categories),
            Line::from(""),
            Line::from(vec![Span::styled("Message: ", styles.text_muted)]),
            Line::from(vec![display_text, Span::styled("_", styles.text_warning)]),
        ];
        frame.render_widget(
            Paragraph::new(content_lines).wrap(Wrap { trim: false }),
            content_area,
        );

        // Render fixed footer
        let sep_width = footer_area.width as usize;
        let footer = Paragraph::new(vec![
            Line::styled("─".repeat(sep_width), styles.text_muted),
            Line::from(vec![
                Span::styled(" Tab ", styles.text_warning),
                Span::styled("category  ", styles.text_muted),
                Span::styled("Enter ", styles.text_warning),
                Span::styled("submit  ", styles.text_muted),
                Span::styled("Esc ", styles.text_warning),
                Span::styled("cancel", styles.text_muted),
            ]),
        ]);
        frame.render_widget(footer, footer_area);
    }

    pub(crate) fn render_terminal_dialog(&mut self, frame: &mut Frame) {
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

        let area = frame.area();
        if area.width < 30 || area.height < 12 {
            return;
        }

        // Copy theme values to avoid borrow conflicts
        let palette_text_primary = self.theme().palette.text_primary;
        let palette_accent = self.theme().palette.accent;
        let bg_secondary = self.theme().styles.bg_secondary;
        let text_warning = self.theme().styles.text_warning;
        let text_muted = self.theme().styles.text_muted;

        // Large dialog - 85% of screen
        let dialog_width = (area.width * 85 / 100)
            .max(60)
            .min(area.width.saturating_sub(2));
        let dialog_height = (area.height * 80 / 100)
            .max(16)
            .min(area.height.saturating_sub(2));
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;
        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);
        self.prune_full_pane_hyperlinks(dialog_area);

        // Clear area behind dialog
        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .title(" Terminal ")
            .title_style(
                Style::default()
                    .fg(palette_text_primary)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette_accent))
            .style(bg_secondary);

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // Resize the shell pane and collect terminal lines
        let content_height = inner.height.saturating_sub(2);
        let content_width = inner.width;

        if let Some(name) = self.terminal_pane_name.clone() {
            // Resize pane to fit dialog
            if let Some(pane) = self.mux.get_mut(&name) {
                let _ = pane.resize(content_height, content_width);
            }

            // Render terminal content
            if let Some(pane) = self.mux.get(&name) {
                let lines: Vec<Line> = (0..content_height)
                    .map(|row| pane.row_as_line(row).unwrap_or_default())
                    .collect();

                let content_area = Rect::new(inner.x, inner.y, content_width, content_height);
                let content = Paragraph::new(lines);
                frame.render_widget(content, content_area);
            }
        }

        // Render footer hints
        let footer_area = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        );
        let footer = Paragraph::new(Line::from(vec![
            Span::styled(" Ctrl+T ", text_warning),
            Span::styled("hide  ", text_muted),
            Span::styled("Ctrl+D ", text_warning),
            Span::styled("kill", text_muted),
        ]));
        frame.render_widget(footer, footer_area);
    }

    pub(crate) fn render_help_overlay(&self, frame: &mut Frame) {
        use ratatui::layout::Alignment;
        use ratatui::style::Style;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

        let area = frame.area();
        let palette = &self.theme().palette;
        let styles = &self.theme().styles;
        let max_width = area.width.saturating_sub(4);
        let max_height = area.height.saturating_sub(4);
        if max_width < 28 || max_height < 8 {
            return;
        }

        let compact = max_height < 30 || max_width < 58;
        let help_width = if compact {
            max_width.clamp(36, 56).min(max_width)
        } else {
            max_width.clamp(50, 72).min(max_width)
        };

        let help_text = if compact {
            vec![
                Line::from(""),
                Line::from(" Worker/Supervisor"),
                Line::from("   typing     -> focused pane"),
                Line::from("   Ctrl+L/R   prev/next pane"),
                Line::from("   Ctrl+P     toggle sidecar"),
                Line::from("   Ctrl+N     resize mode"),
                Line::from("   Ctrl+R     refresh"),
                Line::from("   Ctrl+T     terminal"),
                Line::from("   Ctrl+]     collapse sidecar"),
                Line::from("   Ctrl+Q     quit"),
                Line::from("   F1         toggle help"),
                Line::from(""),
                Line::from(" Sidecar"),
                Line::from("   Enter      open detail"),
                Line::from("   j/k        scroll"),
                Line::from("   f          cycle filter"),
                Line::from("   t/c/v      collapse sections"),
                Line::from("   Space      toggle epic"),
                Line::from("   Esc        back/unfocus"),
                Line::from(""),
                Line::from(" Diff + Mouse"),
                Line::from("   /, n, N    search"),
                Line::from("   click/wheel focus+scroll"),
                Line::from("   F10        toggle SELECT mode"),
                Line::from("              (drag to copy)"),
                Line::from(""),
            ]
        } else {
            vec![
                Line::from(""),
                Line::from(" When WORKER focused:"),
                Line::from("   (typing)   Goes to worker"),
                Line::from("   Ctrl+Left  Previous pane"),
                Line::from("   Ctrl+Right Next pane"),
                Line::from("   Ctrl+P     Toggle sidecar focus"),
                Line::from("   Ctrl+N     Enter resize mode"),
                Line::from("   Ctrl+Q     Quit"),
                Line::from("   Ctrl+R     Refresh Cassy data"),
                Line::from("   Ctrl+T     Open terminal"),
                Line::from("   Ctrl+]     Toggle sidebar collapse"),
                Line::from("   Ctrl+E     Dismiss error banner"),
                Line::from("   PgUp/PgDn  Scroll worker output"),
                Line::from("   F          Send feedback"),
                Line::from(""),
                Line::from(" When SIDECAR focused:"),
                Line::from("   Enter      Open detail view"),
                Line::from("   j/k        Scroll up/down"),
                Line::from("   f          Cycle agent filter"),
                Line::from("   t/c/v      Toggle section collapse"),
                Line::from("   Space      Toggle epic collapse"),
                Line::from("   Tab        Next sidecar panel"),
                Line::from("   Esc        Back / unfocus"),
                Line::from("   F          Send feedback"),
                Line::from(""),
                Line::from(" Mouse:"),
                Line::from("   Click tab  Switch worker tab"),
                Line::from("   Click pane Focus pane"),
                Line::from("   Scroll     Scroll focused pane"),
                Line::from("   F10        Toggle SELECT mode"),
                Line::from("              (drag to copy natively)"),
                Line::from(""),
                Line::from(" In DIFF view:"),
                Line::from("   /          Start search"),
                Line::from("   n/N        Next/prev match"),
                Line::from("   Esc        Exit search / back"),
                Line::from(""),
                Line::from(" When SUPERVISOR focused:"),
                Line::from("   (typing)   Goes to supervisor"),
                Line::from("   Ctrl+Left  Previous pane"),
                Line::from("   Ctrl+Right Next pane"),
                Line::from("   Ctrl+P     Toggle sidecar focus"),
                Line::from("   Ctrl+N     Enter resize mode"),
                Line::from("   Ctrl+Q     Quit"),
                Line::from("   Ctrl+T     Open terminal"),
                Line::from("   Ctrl+R     Refresh Cassy data"),
                Line::from("   Ctrl+]     Toggle sidebar collapse"),
                Line::from("   Ctrl+E     Dismiss error banner"),
                Line::from("   F1         Toggle this help"),
                Line::from(""),
            ]
        };

        use ratatui::layout::{Constraint, Layout};
        use ratatui::widgets::Padding;

        // +2 for borders, +2 for footer
        let desired_height = (help_text.len() as u16).saturating_add(4);
        let help_height = desired_height.min(max_height);
        let x = (area.width.saturating_sub(help_width)) / 2;
        let y = (area.height.saturating_sub(help_height)) / 2;
        let help_area = Rect::new(x, y, help_width, help_height);
        self.prune_full_pane_hyperlinks(help_area);

        frame.render_widget(Clear, help_area);

        let block = Block::default()
            .title(" Help ")
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .style(Style::default().bg(palette.bg_overlay))
            .padding(Padding::horizontal(1));

        let inner = block.inner(help_area);
        frame.render_widget(block, help_area);

        // Split inner area: content + footer
        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);

        let content_area = chunks[0];
        let footer_area = chunks[1];

        let paragraph = Paragraph::new(help_text).alignment(Alignment::Left);
        frame.render_widget(paragraph, content_area);

        // Fixed footer hints
        let sep_width = footer_area.width as usize;
        let footer = Paragraph::new(vec![
            Line::styled("─".repeat(sep_width), styles.text_muted),
            Line::from(vec![
                Span::styled(" Esc ", styles.text_warning),
                Span::styled("close  ", styles.text_muted),
                Span::styled("F1 ", styles.text_warning),
                Span::styled("toggle", styles.text_muted),
            ]),
        ]);
        frame.render_widget(footer, footer_area);
    }
}
