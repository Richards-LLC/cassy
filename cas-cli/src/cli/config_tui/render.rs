use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::cli::config_tui::{ConfigTuiApp, ViewMode};

impl ConfigTuiApp {
    pub(crate) fn render_header(&self, frame: &mut Frame, area: Rect) {
        let title = if self.has_unsaved {
            " Cassy Configuration [*unsaved] "
        } else {
            " Cassy Configuration "
        };

        let header = Paragraph::new(title)
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            );

        frame.render_widget(header, area);
    }

    pub(crate) fn render_body(&mut self, frame: &mut Frame, area: Rect) {
        match self.mode {
            ViewMode::SectionList => self.render_section_list(frame, area),
            ViewMode::SectionEdit | ViewMode::ValueEdit => self.render_section_edit(frame, area),
            ViewMode::DiffView => self.render_diff_view(frame, area),
            _ => self.render_section_list(frame, area),
        }
    }

    fn render_section_list(&mut self, frame: &mut Frame, area: Rect) {
        // Split: section list on left, description on right
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        // Section list
        let items: Vec<ListItem> = self
            .sections
            .iter()
            .map(|s| {
                let modified_marker = if s.modified_count > 0 {
                    format!(" *{}", s.modified_count)
                } else {
                    String::new()
                };

                let line = Line::from(vec![
                    Span::styled(&s.name, Style::default().fg(Color::Yellow)),
                    Span::raw(format!(" ({} options{})", s.option_count, modified_marker)),
                ]);
                ListItem::new(line)
            })
            .collect();

        let sections_list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" Sections "))
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        frame.render_stateful_widget(sections_list, chunks[0], &mut self.section_state);

        // Description panel
        let selected_desc = self
            .section_state
            .selected()
            .and_then(|i| self.sections.get(i))
            .map(|s| {
                format!(
                    "{}\n\n{} options\n{} modified",
                    s.description, s.option_count, s.modified_count
                )
            })
            .unwrap_or_default();

        let desc = Paragraph::new(selected_desc)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Description "),
            )
            .wrap(Wrap { trim: true });

        frame.render_widget(desc, chunks[1]);
    }

    fn render_section_edit(&mut self, frame: &mut Frame, area: Rect) {
        // Split: option list on left, details on right
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        // Option list
        let items: Vec<ListItem> =
            self.option_keys
                .iter()
                .map(|key| {
                    let meta = self.registry.get(key);
                    let value =
                        self.values.get(key.as_str()).cloned().unwrap_or_else(|| {
                            meta.map(|m| m.default.to_string()).unwrap_or_default()
                        });
                    let is_modified = meta.map(|m| m.is_modified(&value)).unwrap_or(false);

                    let key_short = key.split('.').next_back().unwrap_or(key);
                    let modified_marker = if is_modified { "*" } else { " " };

                    let line = Line::from(vec![
                        Span::styled(
                            modified_marker.to_string(),
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::styled(
                            key_short.to_string(),
                            Style::default().fg(if is_modified {
                                Color::Yellow
                            } else {
                                Color::White
                            }),
                        ),
                        Span::raw(" = "),
                        Span::styled(
                            value.clone(),
                            Style::default().fg(if is_modified {
                                Color::Green
                            } else {
                                Color::DarkGray
                            }),
                        ),
                    ]);
                    ListItem::new(line)
                })
                .collect();

        let section_name = self.current_section.as_deref().unwrap_or("Options");
        let options_list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {section_name} ")),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        frame.render_stateful_widget(options_list, chunks[0], &mut self.option_state);

        // Details panel
        let selected_key = self
            .option_state
            .selected()
            .and_then(|i| self.option_keys.get(i));

        let details = if let Some(key) = selected_key {
            if let Some(meta) = self.registry.get(key) {
                let value = self
                    .values
                    .get(key.as_str())
                    .cloned()
                    .unwrap_or_else(|| meta.default.to_string());
                let is_modified = meta.is_modified(&value);

                format!(
                    "{}\n\nKey: {}\nType: {}\n\nCurrent: {}{}\nDefault: {}\n\n{}{}",
                    meta.name,
                    key,
                    meta.value_type.name(),
                    value,
                    if is_modified { " (modified)" } else { "" },
                    meta.default,
                    meta.description,
                    if meta.advanced {
                        "\n\n[Advanced option]"
                    } else {
                        ""
                    }
                )
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let details_widget = Paragraph::new(details)
            .block(Block::default().borders(Borders::ALL).title(" Details "))
            .wrap(Wrap { trim: true });

        frame.render_widget(details_widget, chunks[1]);
    }

    fn render_diff_view(&self, frame: &mut Frame, area: Rect) {
        let mut diff_lines: Vec<Line> = Vec::new();

        for key in self.registry.all_keys() {
            if let Some(meta) = self.registry.get(key) {
                let current = self
                    .values
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| meta.default.to_string());

                if meta.is_modified(&current) {
                    diff_lines.push(Line::from(vec![Span::styled(
                        key.to_string(),
                        Style::default().fg(Color::Yellow),
                    )]));
                    diff_lines.push(Line::from(vec![
                        Span::raw("  - "),
                        Span::styled(meta.default.to_string(), Style::default().fg(Color::Red)),
                    ]));
                    diff_lines.push(Line::from(vec![
                        Span::raw("  + "),
                        Span::styled(current.clone(), Style::default().fg(Color::Green)),
                    ]));
                    diff_lines.push(Line::default());
                }
            }
        }

        if diff_lines.is_empty() {
            diff_lines.push(Line::from("No differences from default configuration"));
        }

        let diff = Paragraph::new(diff_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Differences from Defaults "),
            )
            .wrap(Wrap { trim: true });

        frame.render_widget(diff, area);
    }

    pub(crate) fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let (message, style) = if let Some((msg, is_error)) = &self.status_message {
            (
                msg.clone(),
                if *is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Green)
                },
            )
        } else {
            let help_text = match self.mode {
                ViewMode::SectionList => {
                    "↑↓/jk:Navigate │ Enter:Edit │ d:Diff │ a:Toggle Advanced │ s:Save │ q:Quit │ ?:Help"
                }
                ViewMode::SectionEdit => {
                    "↑↓/jk:Navigate │ Enter:Edit │ r:Reset │ Esc:Back │ s:Save │ q:Quit"
                }
                ViewMode::DiffView => "Esc:Back │ r:Reset All │ q:Quit",
                ViewMode::Help | ViewMode::ResetConfirm | ViewMode::ValueEdit => {
                    "Esc:Cancel │ Enter:Confirm"
                }
            };
            (help_text.to_string(), Style::default().fg(Color::DarkGray))
        };

        let footer = Paragraph::new(message)
            .style(style)
            .block(Block::default().borders(Borders::ALL));

        frame.render_widget(footer, area);
    }

    pub(crate) fn render_help_overlay(&self, frame: &mut Frame, area: Rect) {
        let help_text = r#"
  Cassy Configuration Editor - Help

  Navigation:
    ↑/k       Move up
    ↓/j       Move down
    Enter     Edit selected / Confirm
    Esc       Go back / Cancel

  Actions:
    s         Save configuration
    d         Show diff from defaults
    r         Reset selected option
    a         Toggle advanced options
    q         Quit (prompts if unsaved)
    ?         Show this help

  In Value Edit:
    Enter     Save value
    Esc       Cancel edit
    Backspace Delete character
"#;

        let popup_area = crate::cli::config_tui::centered_rect(60, 80, area);
        frame.render_widget(Clear, popup_area);

        let help = Paragraph::new(help_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Help ")
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false });

        frame.render_widget(help, popup_area);
    }

    pub(crate) fn render_reset_confirm(&self, frame: &mut Frame, area: Rect) {
        let popup_area = crate::cli::config_tui::centered_rect(50, 30, area);
        frame.render_widget(Clear, popup_area);

        let confirm =
            Paragraph::new("\nReset to default value?\n\nPress Enter to confirm, Esc to cancel")
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Confirm Reset ")
                        .border_style(Style::default().fg(Color::Yellow)),
                )
                .alignment(Alignment::Center);

        frame.render_widget(confirm, popup_area);
    }

    pub(crate) fn render_value_edit(&self, frame: &mut Frame, area: Rect) {
        let popup_area = crate::cli::config_tui::centered_rect(60, 40, area);
        frame.render_widget(Clear, popup_area);

        if let Some(edit) = &self.value_edit {
            let meta = self.registry.get(&edit.key);
            let meta_info = meta
                .map(|m| format!("Type: {} | Default: {}", m.value_type.name(), m.default))
                .unwrap_or_default();

            let mut text = vec![
                Line::from(format!("Editing: {}", edit.key)),
                Line::from(meta_info).style(Style::default().fg(Color::DarkGray)),
                Line::default(),
                Line::from(vec![
                    Span::raw("Value: "),
                    Span::styled(
                        &edit.input,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::UNDERLINED),
                    ),
                ]),
            ];

            if let Some(err) = &edit.error {
                text.push(Line::default());
                text.push(Line::from(err.as_str()).style(Style::default().fg(Color::Red)));
            }

            text.push(Line::default());
            text.push(
                Line::from("Enter: Save | Esc: Cancel").style(Style::default().fg(Color::DarkGray)),
            );

            let edit_widget = Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Edit Value ")
                    .border_style(Style::default().fg(Color::Cyan)),
            );

            frame.render_widget(edit_widget, popup_area);
        }
    }
}
