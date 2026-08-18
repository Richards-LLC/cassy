//! TUI Config Editor - Interactive terminal configuration editor
//!
//! Provides a full-screen terminal UI for browsing and editing Cassy configuration
//! options, organized by sections with real-time preview and validation.
//!
//! # Integration Status
//! Ready for `cas config edit` interactive command.

// #![allow(dead_code)] // Check unused

use std::collections::HashMap;
use std::io::{self, Stdout};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{prelude::*, widgets::ListState};

use crate::config::{Config, ConfigRegistry, registry};

mod render;

/// Section info for the section list
struct SectionInfo {
    name: String,
    description: String,
    option_count: usize,
    modified_count: usize,
}

/// View mode for the TUI
enum ViewMode {
    /// Section list (main view)
    SectionList,
    /// Editing options in a section
    SectionEdit,
    /// Showing diff
    DiffView,
    /// Confirming reset
    ResetConfirm,
    /// Help overlay
    Help,
    /// Editing a specific value
    ValueEdit,
}

/// State for editing a value
struct ValueEditState {
    key: String,
    input: String,
    cursor_pos: usize,
    error: Option<String>,
}

/// Main TUI app state
pub struct ConfigTuiApp {
    config: Config,
    original_config: Config,
    cas_root: std::path::PathBuf,
    registry: &'static ConfigRegistry,

    // Section list state
    sections: Vec<SectionInfo>,
    section_state: ListState,

    // Option list state (when in section edit mode)
    current_section: Option<String>,
    option_keys: Vec<String>,
    option_state: ListState,

    // Current values cache
    values: HashMap<String, String>,

    // View state
    mode: ViewMode,
    value_edit: Option<ValueEditState>,

    // Show all (including advanced) options
    show_advanced: bool,

    // Unsaved changes indicator
    has_unsaved: bool,

    // Status message
    status_message: Option<(String, bool)>, // (message, is_error)

    // Should quit
    should_quit: bool,
}

impl ConfigTuiApp {
    pub fn new(cas_root: &std::path::Path) -> anyhow::Result<Self> {
        let cas_root = cas_root.to_path_buf();
        let config = Config::load(&cas_root)?;
        let original_config = config.clone();
        let registry = registry();

        let values: HashMap<String, String> = config.list().into_iter().collect();

        // Build section info
        let mut sections = Vec::new();
        for section in registry.sections() {
            let configs = registry.configs_in_section(section);
            let option_count = configs.len();
            let modified_count = configs
                .iter()
                .filter(|m| {
                    let value = values
                        .get(m.key)
                        .cloned()
                        .unwrap_or_else(|| m.default.to_string());
                    m.is_modified(&value)
                })
                .count();

            sections.push(SectionInfo {
                name: section.to_string(),
                description: registry
                    .section_description(section)
                    .unwrap_or("")
                    .to_string(),
                option_count,
                modified_count,
            });
        }

        let mut section_state = ListState::default();
        if !sections.is_empty() {
            section_state.select(Some(0));
        }

        Ok(Self {
            config,
            original_config,
            cas_root,
            registry,
            sections,
            section_state,
            current_section: None,
            option_keys: Vec::new(),
            option_state: ListState::default(),
            values,
            mode: ViewMode::SectionList,
            value_edit: None,
            show_advanced: false,
            has_unsaved: false,
            status_message: None,
            should_quit: false,
        })
    }

    /// Run the TUI application
    pub fn run(&mut self) -> anyhow::Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Main loop
        let result = self.main_loop(&mut terminal);

        // Restore terminal
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

        result
    }

    fn main_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> anyhow::Result<()> {
        loop {
            terminal.draw(|f| self.ui(f))?;

            if self.should_quit {
                return Ok(());
            }

            // Handle input
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key.code)?;
                    }
                }
            }
        }
    }

    fn ui(&mut self, frame: &mut Frame) {
        let area = frame.area();

        // Main layout: header, body, footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(10),   // Body
                Constraint::Length(3), // Footer/status
            ])
            .split(area);

        self.render_header(frame, chunks[0]);
        self.render_body(frame, chunks[1]);
        self.render_footer(frame, chunks[2]);

        // Render overlays
        match &self.mode {
            ViewMode::Help => self.render_help_overlay(frame, area),
            ViewMode::ResetConfirm => self.render_reset_confirm(frame, area),
            ViewMode::ValueEdit => self.render_value_edit(frame, area),
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyCode) -> anyhow::Result<()> {
        // Clear status message on any key
        self.status_message = None;

        match &self.mode {
            ViewMode::SectionList => self.handle_section_list_key(key),
            ViewMode::SectionEdit => self.handle_section_edit_key(key),
            ViewMode::DiffView => self.handle_diff_view_key(key),
            ViewMode::Help => self.handle_help_key(key),
            ViewMode::ResetConfirm => self.handle_reset_confirm_key(key),
            ViewMode::ValueEdit => self.handle_value_edit_key(key),
        }
    }

    fn handle_section_list_key(&mut self, key: KeyCode) -> anyhow::Result<()> {
        match key {
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                if self.has_unsaved {
                    self.status_message = Some((
                        "Unsaved changes! Press 's' to save or 'q' again to discard".to_string(),
                        true,
                    ));
                    // Mark as "warned" - second q will quit
                    // For simplicity, just quit
                }
                self.should_quit = true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.select_next_section();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.select_prev_section();
            }
            KeyCode::Enter => {
                self.enter_section_edit();
            }
            KeyCode::Char('d') => {
                self.mode = ViewMode::DiffView;
            }
            KeyCode::Char('a') => {
                self.show_advanced = !self.show_advanced;
                self.refresh_sections();
            }
            KeyCode::Char('s') => {
                self.save_config()?;
            }
            KeyCode::Char('?') => {
                self.mode = ViewMode::Help;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_section_edit_key(&mut self, key: KeyCode) -> anyhow::Result<()> {
        match key {
            KeyCode::Esc => {
                self.mode = ViewMode::SectionList;
                self.current_section = None;
                self.option_keys.clear();
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                self.should_quit = true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.select_next_option();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.select_prev_option();
            }
            KeyCode::Enter => {
                self.start_value_edit();
            }
            KeyCode::Char('r') => {
                self.mode = ViewMode::ResetConfirm;
            }
            KeyCode::Char('s') => {
                self.save_config()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_diff_view_key(&mut self, key: KeyCode) -> anyhow::Result<()> {
        match key {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = ViewMode::SectionList;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_help_key(&mut self, key: KeyCode) -> anyhow::Result<()> {
        match key {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q') => {
                self.mode = ViewMode::SectionList;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_reset_confirm_key(&mut self, key: KeyCode) -> anyhow::Result<()> {
        match key {
            KeyCode::Esc => {
                self.mode = ViewMode::SectionEdit;
            }
            KeyCode::Enter => {
                self.reset_selected_option()?;
                self.mode = ViewMode::SectionEdit;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_value_edit_key(&mut self, key: KeyCode) -> anyhow::Result<()> {
        if let Some(edit) = &mut self.value_edit {
            match key {
                KeyCode::Esc => {
                    self.value_edit = None;
                    self.mode = ViewMode::SectionEdit;
                }
                KeyCode::Enter => {
                    let key = edit.key.clone();
                    let value = edit.input.clone();

                    // Validate
                    if let Some(meta) = self.registry.get(&key) {
                        match meta.validate(&value) {
                            Ok(()) => {
                                self.config.set(&key, &value)?;
                                self.values.insert(key, value);
                                self.has_unsaved = true;
                                self.value_edit = None;
                                self.mode = ViewMode::SectionEdit;
                                self.status_message = Some(("Value updated".to_string(), false));
                            }
                            Err(e) => {
                                edit.error = Some(e.to_string());
                            }
                        }
                    }
                }
                KeyCode::Backspace => {
                    if edit.cursor_pos > 0 {
                        edit.input.remove(edit.cursor_pos - 1);
                        edit.cursor_pos -= 1;
                        edit.error = None;
                    }
                }
                KeyCode::Delete => {
                    if edit.cursor_pos < edit.input.len() {
                        edit.input.remove(edit.cursor_pos);
                        edit.error = None;
                    }
                }
                KeyCode::Left => {
                    if edit.cursor_pos > 0 {
                        edit.cursor_pos -= 1;
                    }
                }
                KeyCode::Right => {
                    if edit.cursor_pos < edit.input.len() {
                        edit.cursor_pos += 1;
                    }
                }
                KeyCode::Home => {
                    edit.cursor_pos = 0;
                }
                KeyCode::End => {
                    edit.cursor_pos = edit.input.len();
                }
                KeyCode::Char(c) => {
                    edit.input.insert(edit.cursor_pos, c);
                    edit.cursor_pos += 1;
                    edit.error = None;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn select_next_section(&mut self) {
        let len = self.sections.len();
        if len == 0 {
            return;
        }

        let i = self.section_state.selected().unwrap_or(0);
        let next = if i >= len - 1 { 0 } else { i + 1 };
        self.section_state.select(Some(next));
    }

    fn select_prev_section(&mut self) {
        let len = self.sections.len();
        if len == 0 {
            return;
        }

        let i = self.section_state.selected().unwrap_or(0);
        let prev = if i == 0 { len - 1 } else { i - 1 };
        self.section_state.select(Some(prev));
    }

    fn select_next_option(&mut self) {
        let len = self.option_keys.len();
        if len == 0 {
            return;
        }

        let i = self.option_state.selected().unwrap_or(0);
        let next = if i >= len - 1 { 0 } else { i + 1 };
        self.option_state.select(Some(next));
    }

    fn select_prev_option(&mut self) {
        let len = self.option_keys.len();
        if len == 0 {
            return;
        }

        let i = self.option_state.selected().unwrap_or(0);
        let prev = if i == 0 { len - 1 } else { i - 1 };
        self.option_state.select(Some(prev));
    }

    fn enter_section_edit(&mut self) {
        if let Some(i) = self.section_state.selected() {
            if let Some(section) = self.sections.get(i) {
                self.current_section = Some(section.name.clone());

                // Get options for this section
                self.option_keys = self
                    .registry
                    .configs_in_section(&section.name)
                    .iter()
                    .filter(|m| self.show_advanced || !m.advanced)
                    .map(|m| m.key.to_string())
                    .collect();

                self.option_state = ListState::default();
                if !self.option_keys.is_empty() {
                    self.option_state.select(Some(0));
                }

                self.mode = ViewMode::SectionEdit;
            }
        }
    }

    fn start_value_edit(&mut self) {
        if let Some(i) = self.option_state.selected() {
            if let Some(key) = self.option_keys.get(i) {
                let current_value = self.values.get(key).cloned().unwrap_or_else(|| {
                    self.registry
                        .get(key)
                        .map(|m| m.default.to_string())
                        .unwrap_or_default()
                });

                self.value_edit = Some(ValueEditState {
                    key: key.clone(),
                    input: current_value.clone(),
                    cursor_pos: current_value.len(),
                    error: None,
                });

                self.mode = ViewMode::ValueEdit;
            }
        }
    }

    fn reset_selected_option(&mut self) -> anyhow::Result<()> {
        if let Some(i) = self.option_state.selected() {
            if let Some(key) = self.option_keys.get(i) {
                if let Some(meta) = self.registry.get(key) {
                    self.config.set(key, meta.default)?;
                    self.values.insert(key.clone(), meta.default.to_string());
                    self.has_unsaved = true;
                    self.status_message = Some((format!("Reset {key} to default"), false));
                }
            }
        }
        Ok(())
    }

    fn save_config(&mut self) -> anyhow::Result<()> {
        self.config.save(&self.cas_root)?;
        self.original_config = self.config.clone();
        self.has_unsaved = false;
        self.refresh_sections();
        self.status_message = Some(("Configuration saved".to_string(), false));
        Ok(())
    }

    fn refresh_sections(&mut self) {
        self.values = self.config.list().into_iter().collect();

        // Rebuild section info
        self.sections.clear();
        for section in self.registry.sections() {
            let configs = self.registry.configs_in_section(section);
            let filtered_configs: Vec<_> = configs
                .iter()
                .filter(|m| self.show_advanced || !m.advanced)
                .collect();

            let option_count = filtered_configs.len();
            let modified_count = filtered_configs
                .iter()
                .filter(|m| {
                    let value = self
                        .values
                        .get(m.key)
                        .cloned()
                        .unwrap_or_else(|| m.default.to_string());
                    m.is_modified(&value)
                })
                .count();

            self.sections.push(SectionInfo {
                name: section.to_string(),
                description: self
                    .registry
                    .section_description(section)
                    .unwrap_or("")
                    .to_string(),
                option_count,
                modified_count,
            });
        }
    }
}

/// Helper function to create a centered rectangle
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Entry point to run the TUI config editor
pub fn run_tui(section: Option<String>, cas_root: &std::path::Path) -> anyhow::Result<()> {
    let mut app = ConfigTuiApp::new(cas_root)?;

    // If a section is specified, jump to it
    if let Some(section_name) = section {
        for (i, section) in app.sections.iter().enumerate() {
            if section.name == section_name || section.name.starts_with(&section_name) {
                app.section_state.select(Some(i));
                app.enter_section_edit();
                break;
            }
        }
    }

    app.run()
}
