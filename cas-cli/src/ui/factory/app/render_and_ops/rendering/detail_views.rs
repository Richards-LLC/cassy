use crate::ui::factory::app::imports::*;

impl FactoryApp {
    pub(crate) fn render_task_detail(&self, frame: &mut Frame, area: Rect, task_id: &str) {
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

        let palette = &self.theme().palette;
        let styles = &self.theme().styles;

        // Find the task
        let task = self
            .director_data
            .in_progress_tasks
            .iter()
            .chain(self.director_data.ready_tasks.iter())
            .find(|t| t.id == task_id);

        let Some(task) = task else {
            // Task not found
            let block = Block::default()
                .title(" Task Detail ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(styles.border_muted);
            let para = Paragraph::new("Task not found").block(block);
            frame.render_widget(para, area);
            return;
        };

        // Build content lines
        let mut lines = Vec::new();
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" ID: ", styles.text_muted),
            Span::styled(&task.id, styles.entity_id),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" Title: ", styles.text_muted),
            Span::styled(
                &task.title,
                styles.text_primary.add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" Status: ", styles.text_muted),
            Span::styled(
                format!("{:?}", task.status),
                Style::default().fg(match task.status {
                    cas_types::TaskStatus::Open => palette.task_open,
                    cas_types::TaskStatus::InProgress => palette.task_in_progress,
                    cas_types::TaskStatus::Closed => palette.task_closed,
                    cas_types::TaskStatus::Cancelled => palette.task_closed,
                    cas_types::TaskStatus::Blocked => palette.task_blocked,
                    cas_types::TaskStatus::AwaitingMerge => palette.task_blocked,
                }),
            ),
        ]));

        if let Some(assignee) = &task.assignee {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(" Assignee: ", styles.text_muted),
                Span::styled(assignee, styles.text_success),
            ]));
        }

        // Priority
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" Priority: ", styles.text_muted),
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

        // Task type
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" Type: ", styles.text_muted),
            Span::styled(format!("{:?}", task.task_type), styles.text_primary),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" [Esc] back  ", styles.text_muted),
            Span::styled("[j/k] scroll", styles.text_muted),
        ]));

        let block = Block::default()
            .title(" Task Detail ")
            .title_style(
                Style::default()
                    .fg(palette.status_info)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.status_info));

        // Apply scroll offset
        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(self.detail_scroll as usize)
            .collect();

        let para = Paragraph::new(visible_lines)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(para, area);
    }

    /// Render activity log view
    pub(crate) fn render_activity_log(&self, frame: &mut Frame, area: Rect) {
        use crate::ui::widgets::format_activity_summary;
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

        let palette = &self.theme().palette;
        let styles = &self.theme().styles;
        let activity_backend_tags = self.activity_backend_tags();
        let mut lines = Vec::new();
        lines.push(Line::from(""));

        for event in &self.director_data.activity {
            let time_str = event.created_at.format("%H:%M:%S").to_string();
            let event_type = format!("{:?}", event.event_type);

            lines.push(Line::from(vec![
                Span::styled(format!(" {time_str} "), styles.text_muted),
                Span::styled(event_type, Style::default().fg(palette.status_warning)),
            ]));
            if !event.summary.is_empty() {
                lines.push(Line::from(format!(
                    "   {}",
                    format_activity_summary(event, &activity_backend_tags)
                )));
            }
        }

        if lines.len() == 1 {
            lines.push(Line::from("   No activity recorded"));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" [Esc] back  ", styles.text_muted),
            Span::styled("[j/k] scroll", styles.text_muted),
        ]));

        let block = Block::default()
            .title(" Activity Log ")
            .title_style(
                Style::default()
                    .fg(palette.status_info)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.status_info));

        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(self.detail_scroll as usize)
            .collect();

        let para = Paragraph::new(visible_lines).block(block);
        frame.render_widget(para, area);
    }

    /// Render file diff view using cas-diffs DiffWidget with search highlighting.
    pub(crate) fn render_file_diff(&mut self, frame: &mut Frame, area: Rect, file_path: &str) {
        use cas_diffs::iter::DiffStyle;
        use cas_diffs::widget::DiffWidget;
        use ratatui::layout::{Constraint, Direction, Layout};
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Borders, Paragraph, StatefulWidget};

        // Copy styles we need up front to avoid borrow conflicts with diff_view_state
        let text_muted = self.theme().styles.text_muted;
        let text_warning = self.theme().styles.text_warning;
        let text_primary = self.theme().styles.text_primary;
        let bg_elevated = self.theme().styles.bg_elevated;
        let palette_text_primary = self.theme().palette.text_primary;
        let palette_status_success = self.theme().palette.status_success;
        let palette_status_error = self.theme().palette.status_error;
        let palette_status_warning = self.theme().palette.status_warning;
        let palette_status_info = self.theme().palette.status_info;

        let style_label = match self.diff_display_style {
            DiffStyle::Split => "split",
            _ => "unified",
        };

        // Build title spans
        let mut title_spans = vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                file_path,
                Style::default()
                    .fg(palette_text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" [{style_label}] "), text_muted),
        ];

        // Add stats from parsed metadata
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

        // Add search match info if searching
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

        title_spans.push(Span::styled(" /:search  ]/[:hunk  s:split ", text_muted));

        let title = Line::from(title_spans);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette_status_info));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Reserve space for search input if in search mode
        let (diff_area, search_area) = if self.diff_search_mode {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(inner);
            (chunks[0], Some(chunks[1]))
        } else {
            (inner, None)
        };

        // Render using DiffWidget if metadata is available
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
            let search_widget = Paragraph::new(search_line).style(bg_elevated);
            frame.render_widget(search_widget, search_rect);
        }

        // Scroll indicator (bottom right of border)
        let total_lines =
            self.diff_metadata
                .as_ref()
                .map_or(0, |d| match self.diff_display_style {
                    DiffStyle::Split => d.split_line_count,
                    _ => d.unified_line_count,
                });
        let visible_height = diff_area.height as usize;
        if total_lines > visible_height && !self.diff_search_mode {
            let start = self.diff_view_state.scroll_offset;
            let progress = if total_lines > visible_height {
                ((start as f64 / (total_lines - visible_height) as f64) * 100.0) as u16
            } else {
                100
            };
            let scroll_info = format!(" {progress}% ");
            let info_x = area.x + area.width.saturating_sub(scroll_info.len() as u16 + 1);
            let info_y = area.y + area.height.saturating_sub(1);
            let info_area = Rect::new(info_x, info_y, scroll_info.len() as u16, 1);
            let info_widget = Paragraph::new(scroll_info).style(text_muted);
            frame.render_widget(info_widget, info_area);
        }
    }
}
