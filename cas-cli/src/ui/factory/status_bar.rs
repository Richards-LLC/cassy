//! Status bar rendering for the factory TUI

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde::{Deserialize, Serialize};

use crate::ui::factory::app::FactoryApp;
use crate::ui::factory::app::truncate_branch_middle;
use crate::ui::factory::input::InputMode;
use crate::ui::theme::Styles;
use cas_mux::{PaneKind, SupervisorCli};

/// Status bar widget
pub struct StatusBar;

const UPDATE_CHECK_CACHE_RELATIVE: &str = "cache/update-check.json";
const UPDATE_CHECK_CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const UPDATE_CHECK_CACHE_FAILURE_TTL_SECS: u64 = 60 * 60;
const UPDATE_CHECK_TIMEOUT_MS: u64 = 1200;
const UPDATE_CHECK_URL: &str = "https://api.github.com/repos/pippenz/cas/releases/latest";

static UPDATE_REFRESH_IN_FLIGHT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct UpdateCheckCache {
    checked_at_unix: u64,
    latest_version: Option<String>,
    update_available: bool,
    failed: bool,
}

impl StatusBar {
    /// Render the status bar
    pub fn render(frame: &mut Frame, area: Rect, app: &FactoryApp) {
        let palette = &app.theme().palette;
        let styles = &app.theme().styles;
        let mut left_spans = Vec::new();
        let mut right_spans = Vec::new();

        // Left side: Mode indicator
        let mode = match &app.input_mode {
            InputMode::Normal => Span::styled(
                " NORMAL ",
                Style::default().fg(palette.chip_fg).bg(palette.status_info),
            ),
            InputMode::Inject => Span::styled(
                " INJECT ",
                Style::default()
                    .fg(palette.chip_fg)
                    .bg(palette.status_warning),
            ),
            InputMode::PaneSelect => Span::styled(
                " PANE ",
                Style::default().fg(palette.chip_fg).bg(palette.accent),
            ),
            InputMode::Feedback => Span::styled(
                " FEEDBACK ",
                Style::default()
                    .fg(palette.chip_fg)
                    .bg(palette.status_warning),
            ),
            InputMode::Resize => Span::styled(
                " RESIZE ",
                Style::default()
                    .fg(palette.chip_fg)
                    .bg(palette.status_success),
            ),
            InputMode::Terminal => Span::styled(
                " TERMINAL ",
                Style::default().fg(palette.chip_fg).bg(palette.accent),
            ),
        };
        left_spans.push(mode);
        left_spans.push(Span::raw(" "));

        // Show the actual account used by the focused Claude pane. Worker panes may
        // deliberately run with a different config directory than the factory
        // supervisor, so read their resolved WorkerSpec rather than the parent env.
        if let Some(profile) = Self::focused_claude_profile(app) {
            let is_alt = profile != "main";
            let profile_style = if is_alt {
                Style::default()
                    .fg(palette.chip_fg)
                    .bg(palette.status_warning)
                    .add_modifier(Modifier::BOLD)
            } else {
                styles.text_muted
            };
            left_spans.push(Span::styled(
                format!(" CLAUDE {profile} "),
                profile_style,
            ));
            left_spans.push(Span::raw(" "));
        }

        // SELECT MODE indicator (mouse capture disabled for native drag-select)
        if app.select_mode {
            left_spans.push(Span::styled(
                " SELECT MODE — F10 to exit ",
                Style::default()
                    .fg(palette.chip_fg)
                    .bg(palette.status_warning)
                    .add_modifier(Modifier::BOLD),
            ));
            left_spans.push(Span::raw(" "));
        }

        // Update indicator (if cached check says update is available)
        if area.width >= 70 {
            if let Some(update_label) = Self::update_badge(app.cas_dir()) {
                left_spans.push(Span::styled(
                    update_label,
                    styles.text_warning.add_modifier(Modifier::BOLD),
                ));
                left_spans.push(Span::raw(" "));
            }
        }

        // Show layout percentages in resize mode
        if app.input_mode.is_resize() {
            let sizes = app.layout_sizes.unwrap_or_default();
            left_spans.push(Span::styled(
                format!(
                    "[W:{}% S:{}% D:{}%]",
                    sizes.workers, sizes.supervisor, sizes.sidecar
                ),
                styles.text_accent,
            ));
            left_spans.push(Span::raw(" "));
        }

        // Spawning indicator
        if app.spawning_count > 0 {
            // Animated spinner using frame count derived from time
            let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let tick = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 100) as usize;
            let spinner = spinner_chars[tick % spinner_chars.len()];
            left_spans.push(Span::styled(
                format!(
                    "{} Spawning {} worker{}...",
                    spinner,
                    app.spawning_count,
                    if app.spawning_count > 1 { "s" } else { "" }
                ),
                styles.text_warning,
            ));
            left_spans.push(Span::raw(" "));
        }

        // Background terminal indicator
        if app.has_background_terminal() {
            left_spans.push(Span::styled(
                " ● TERM ",
                Style::default()
                    .fg(palette.chip_fg)
                    .bg(palette.accent_dim)
                    .add_modifier(Modifier::BOLD),
            ));
            left_spans.push(Span::raw(" "));
        }

        // Focus indicator
        if app.is_mission_control() {
            // Mission Control mode indicator
            left_spans.push(Span::styled(
                "DASHBOARD",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            let mc_label = app.mc_focus.label();
            if !mc_label.is_empty() {
                left_spans.push(Span::raw(" │ "));
                left_spans.push(Span::styled(
                    mc_label,
                    styles.text_success.add_modifier(Modifier::BOLD),
                ));
            }
        } else if let Some(pane) = app.mux.focused() {
            let (kind_str, kind_color) = match pane.kind() {
                PaneKind::Worker => ("WORKER", palette.accent),
                PaneKind::Supervisor => ("SUP", palette.status_info),
                PaneKind::Director => ("DIR", palette.status_warning),
                PaneKind::Shell => ("SHELL", palette.text_primary),
            };
            left_spans.push(Span::styled(
                pane.id(),
                Style::default().fg(kind_color).add_modifier(Modifier::BOLD),
            ));
            left_spans.push(Span::styled(format!(" [{kind_str}]"), styles.text_muted));
            if let Some(branch_label) =
                Self::branch_label_for_width(app.focused_pane_branch().as_deref(), area.width)
            {
                left_spans.push(Span::raw(" │ "));
                left_spans.push(Span::styled(branch_label, styles.text_muted));
            }
        }

        // Sidecar focus indicator
        if !app.is_mission_control() && app.sidecar_is_focused() {
            left_spans.push(Span::raw(" │ "));
            left_spans.push(Span::styled("SIDECAR", styles.text_success));

            // Show filter if active
            if app.agent_filter.is_some() {
                left_spans.push(Span::styled(
                    format!(" [f:{}]", app.filter_display()),
                    styles.text_warning,
                ));
            }
        }

        // Error message (if any)
        if let Some(error) = &app.error_message {
            let max_error_width = if area.width >= 120 {
                40
            } else if area.width >= 90 {
                26
            } else {
                16
            };
            let error_display = Self::truncate_with_ellipsis(error, max_error_width);
            left_spans.push(Span::raw(" │ "));
            left_spans.push(Span::styled(error_display, styles.text_error));
        }

        // Right side: Keyboard shortcuts (context-sensitive, compact)
        let has_workers = app.worker_count() > 0;
        let sidecar_focused = app.sidecar_is_focused();
        let input_focused = app.focused_accepts_input() && !sidecar_focused;
        let is_pane_select = app.input_mode.is_pane_select();
        let is_resize = app.input_mode.is_resize();
        let is_mc = app.is_mission_control();

        // Build shortcut hints based on available width and focus context
        if area.width >= 80 {
            if is_mc {
                // Mission Control mode shortcuts
                Self::add_shortcut(&mut right_spans, styles, "^W", " panes", styles.hint_info);
                right_spans.push(Span::raw(" │ "));
                Self::add_shortcut(
                    &mut right_spans,
                    styles,
                    "Tab",
                    " focus",
                    styles.hint_accent,
                );
                right_spans.push(Span::raw(" │ "));
                right_spans.push(Span::styled("j/k", styles.hint_info));
                right_spans.push(Span::styled(" scroll", styles.text_muted));
                right_spans.push(Span::raw(" │ "));
                Self::add_shortcut(
                    &mut right_spans,
                    styles,
                    "Enter",
                    " detail",
                    styles.hint_success,
                );
                right_spans.push(Span::raw(" │ "));
                right_spans.push(Span::styled("f/t/c/a", styles.hint_warning));
                right_spans.push(Span::styled(" jump", styles.text_muted));
                right_spans.push(Span::raw(" │ "));
                Self::add_shortcut(
                    &mut right_spans,
                    styles,
                    "i",
                    " inject",
                    styles.hint_success,
                );
                right_spans.push(Span::raw(" │ "));
                right_spans.push(Span::styled(
                    "q",
                    styles.hint_error.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" quit", styles.text_muted));
            } else if is_resize {
                // Resize mode: show resize hints
                right_spans.push(Span::styled(
                    "h/l",
                    styles.hint_accent.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" workers", styles.text_muted));
                right_spans.push(Span::raw(" │ "));

                right_spans.push(Span::styled(
                    "j/k",
                    styles.hint_accent.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" sidecar", styles.text_muted));
                right_spans.push(Span::raw(" │ "));

                right_spans.push(Span::styled(
                    "r",
                    styles.hint_warning.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" reset", styles.text_muted));
                right_spans.push(Span::raw(" │ "));

                right_spans.push(Span::styled(
                    "Esc",
                    styles.hint_success.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" done", styles.text_muted));
            } else if is_pane_select {
                // PaneSelect mode: show navigation hints
                right_spans.push(Span::styled(
                    "hjkl",
                    styles.hint_accent.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" move", styles.text_muted));
                right_spans.push(Span::raw(" │ "));

                right_spans.push(Span::styled(
                    "Enter",
                    styles.hint_success.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" copy", styles.text_muted));
                right_spans.push(Span::raw(" │ "));

                right_spans.push(Span::styled(
                    "Esc",
                    styles.hint_warning.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" exit", styles.text_muted));
            } else if sidecar_focused {
                // Sidecar focused: show sidecar navigation keys
                {
                    use crate::ui::factory::renderer::FactoryViewMode;
                    let view_hint = match app.factory_view_mode {
                        FactoryViewMode::Panes => "^W:Dashboard",
                        FactoryViewMode::MissionControl => "^W:Panes",
                    };
                    right_spans.push(Span::styled(
                        view_hint,
                        styles.hint_info.add_modifier(Modifier::BOLD),
                    ));
                    right_spans.push(Span::raw(" │ "));
                }
                right_spans.push(Span::styled(
                    "Enter",
                    styles.hint_success.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" detail", styles.text_muted));
                right_spans.push(Span::raw(" │ "));
                right_spans.push(Span::styled("j/k", styles.hint_info));
                right_spans.push(Span::styled(" scroll", styles.text_muted));
                right_spans.push(Span::raw(" │ "));
                right_spans.push(Span::styled("f", styles.hint_warning));
                right_spans.push(Span::styled(" filter", styles.text_muted));
                right_spans.push(Span::raw(" │ "));
                right_spans.push(Span::styled("t/c/v", styles.hint_accent));
                right_spans.push(Span::styled(" sections", styles.text_muted));
                right_spans.push(Span::raw(" │ "));
                right_spans.push(Span::styled("Esc", styles.hint_warning));
                right_spans.push(Span::styled(" back", styles.text_muted));
                right_spans.push(Span::raw(" │ "));
                Self::add_shortcut(&mut right_spans, styles, "?", " help", styles.text_primary);
                right_spans.push(Span::raw(" │ "));
                right_spans.push(Span::styled(
                    "q",
                    styles.hint_error.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" quit", styles.text_muted));
            } else if input_focused {
                // Input focused: keys go to pane, show Ctrl combos
                right_spans.push(Span::styled(
                    "^P",
                    styles.hint_info.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" focus", styles.text_muted));
                right_spans.push(Span::raw(" │ "));

                right_spans.push(Span::styled("^N", styles.hint_success));
                right_spans.push(Span::styled(" resize", styles.text_muted));
                right_spans.push(Span::raw(" │ "));

                right_spans.push(Span::styled("^R", styles.hint_warning));
                right_spans.push(Span::styled(" refresh", styles.text_muted));
                right_spans.push(Span::raw(" │ "));

                right_spans.push(Span::styled("^G", styles.hint_accent));
                right_spans.push(Span::styled(" term", styles.text_muted));
                right_spans.push(Span::raw(" │ "));
                Self::add_shortcut(&mut right_spans, styles, "^]", " sidecar", styles.hint_info);
                right_spans.push(Span::raw(" │ "));
                Self::add_shortcut(
                    &mut right_spans,
                    styles,
                    "^W",
                    " dashboard",
                    styles.hint_accent,
                );
                right_spans.push(Span::raw(" │ "));

                right_spans.push(Span::styled("^Q", styles.hint_error));
                right_spans.push(Span::styled(" quit", styles.text_muted));
                if app.error_message.is_some() {
                    right_spans.push(Span::raw(" │ "));
                    right_spans.push(Span::styled("^E", styles.hint_warning));
                    right_spans.push(Span::styled(" dismiss err", styles.text_muted));
                }
            } else {
                // Worker focused: single-key shortcuts work
                Self::add_shortcut(&mut right_spans, styles, "s", " sup", styles.hint_info);
                right_spans.push(Span::raw(" │ "));

                if has_workers {
                    Self::add_shortcut(
                        &mut right_spans,
                        styles,
                        "1-9",
                        " workers",
                        styles.hint_accent,
                    );
                    right_spans.push(Span::raw(" │ "));
                }

                Self::add_shortcut(
                    &mut right_spans,
                    styles,
                    "i",
                    " inject",
                    styles.hint_success,
                );
                right_spans.push(Span::raw(" │ "));
                Self::add_shortcut(&mut right_spans, styles, "^]", " sidecar", styles.hint_info);
                right_spans.push(Span::raw(" │ "));
                Self::add_shortcut(
                    &mut right_spans,
                    styles,
                    "^W",
                    " dashboard",
                    styles.hint_accent,
                );
                right_spans.push(Span::raw(" │ "));
                Self::add_shortcut(&mut right_spans, styles, "?", " help", styles.text_primary);
                right_spans.push(Span::raw(" │ "));
                right_spans.push(Span::styled(
                    "q",
                    styles.hint_error.add_modifier(Modifier::BOLD),
                ));
                right_spans.push(Span::styled(" quit", styles.text_muted));
                if app.error_message.is_some() {
                    right_spans.push(Span::raw(" │ "));
                    right_spans.push(Span::styled("^E", styles.hint_warning));
                    right_spans.push(Span::styled(" dismiss err", styles.text_muted));
                }
            }
        } else if area.width >= 50 {
            // Compact hints
            if is_mc {
                right_spans.push(Span::styled("^W", styles.hint_info));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("Tab", styles.hint_accent));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("j/k", styles.hint_info));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("Enter", styles.hint_success));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("i", styles.hint_success));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("q", styles.hint_error));
            } else if is_resize {
                right_spans.push(Span::styled("h/l", styles.hint_accent));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("j/k", styles.hint_accent));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("r", styles.hint_warning));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("Esc", styles.hint_success));
            } else if is_pane_select {
                right_spans.push(Span::styled("hjkl", styles.hint_accent));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("Enter", styles.hint_success));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("Esc", styles.hint_warning));
            } else if sidecar_focused {
                right_spans.push(Span::styled("Enter", styles.hint_success));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("j/k", styles.hint_info));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("f", styles.hint_warning));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("t/c/v", styles.hint_accent));
                right_spans.push(Span::raw(" "));
                Self::add_shortcut(&mut right_spans, styles, "?", "", styles.text_primary);
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("q", styles.hint_error));
            } else if input_focused {
                right_spans.push(Span::styled("^P", styles.hint_info));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("^N", styles.hint_success));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("^R", styles.hint_warning));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("^G", styles.hint_accent));
                right_spans.push(Span::raw(" "));
                Self::add_shortcut(&mut right_spans, styles, "^]", "", styles.hint_info);
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("^W", styles.hint_accent));
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("^Q", styles.hint_error));
                if app.error_message.is_some() {
                    right_spans.push(Span::raw(" "));
                    right_spans.push(Span::styled("^E", styles.hint_warning));
                }
            } else {
                Self::add_shortcut(&mut right_spans, styles, "s", "", styles.hint_info);
                right_spans.push(Span::raw(" "));
                if has_workers {
                    right_spans.push(Span::styled("1-9", styles.hint_accent));
                    right_spans.push(Span::raw(" "));
                }
                Self::add_shortcut(&mut right_spans, styles, "i", "", styles.hint_success);
                right_spans.push(Span::raw(" "));
                Self::add_shortcut(&mut right_spans, styles, "^]", "", styles.hint_info);
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("^W", styles.hint_accent));
                right_spans.push(Span::raw(" "));
                Self::add_shortcut(&mut right_spans, styles, "?", "", styles.text_primary);
                right_spans.push(Span::raw(" "));
                right_spans.push(Span::styled("q", styles.hint_error));
                if app.error_message.is_some() {
                    right_spans.push(Span::raw(" "));
                    right_spans.push(Span::styled("^E", styles.hint_warning));
                }
            }
        } else {
            // Minimal
            if is_mc {
                right_spans.push(Span::styled("^W Tab jk Enter i q", styles.text_muted));
            } else if is_resize {
                right_spans.push(Span::styled("hjkl r Esc", styles.hint_accent));
            } else if is_pane_select {
                right_spans.push(Span::styled("hjkl Esc", styles.hint_accent));
            } else if sidecar_focused {
                right_spans.push(Span::styled("Enter jk Esc ? q", styles.text_muted));
            } else if input_focused {
                if app.error_message.is_some() {
                    right_spans.push(Span::styled("Tab ^N ^] ^W ^Q ^E", styles.text_muted));
                } else {
                    right_spans.push(Span::styled("Tab ^N ^] ^W ^Q", styles.text_muted));
                }
            } else if app.error_message.is_some() {
                right_spans.push(Span::styled("s ^] ^W ? q ^E", styles.text_muted));
            } else {
                right_spans.push(Span::styled("s ^] ^W ? q", styles.text_muted));
            }
        }

        right_spans.push(Span::raw(" "));

        // Keep right hints within visible width (especially 80-col tmux panes).
        let available = area.width as usize;
        let left_width = Self::spans_display_width(&left_spans);
        let right_budget = available.saturating_sub(left_width + 1);
        Self::trim_spans_from_front(&mut right_spans, right_budget);
        let right_width = Self::spans_display_width(&right_spans);
        let padding = available.saturating_sub(left_width + right_width);

        // Build the complete line
        let mut all_spans = left_spans;
        all_spans.push(Span::raw(" ".repeat(padding)));
        all_spans.extend(right_spans);

        let paragraph = Paragraph::new(Line::from(all_spans)).style(styles.bg_elevated);
        frame.render_widget(paragraph, area);
    }

    fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
        let count = text.chars().count();
        if count <= max_chars {
            return text.to_string();
        }
        if max_chars <= 1 {
            return "…".to_string();
        }
        let keep = max_chars - 1;
        let prefix: String = text.chars().take(keep).collect();
        format!("{prefix}…")
    }

    fn branch_label_for_width(branch: Option<&str>, width: u16) -> Option<String> {
        let branch = branch.filter(|branch| !branch.is_empty())?;
        let max_branch_chars = match width {
            0..=49 => return None,
            50..=69 => 12,
            70..=99 => 20,
            _ => 32,
        };
        Some(format!(
            "branch {}",
            truncate_branch_middle(branch, max_branch_chars)
        ))
    }

    /// Return the account profile represented by a Claude config directory.
    ///
    /// The default config directory is always the main account. Alternate
    /// profiles follow Claude Code's conventional `~/.claude-<profile>` name.
    fn claude_profile_from_config_dir(config_dir: Option<&str>) -> String {
        let config_dir = config_dir.map(str::trim).filter(|dir| !dir.is_empty());
        let Some(config_dir) = config_dir else {
            return "main".to_string();
        };

        let directory_name = config_dir.trim_end_matches('/').rsplit('/').next();
        match directory_name {
            Some(".claude") | None => "main".to_string(),
            Some(name) => name
                .strip_prefix(".claude-")
                .filter(|profile| !profile.is_empty())
                .unwrap_or("main")
                .to_string(),
        }
    }

    /// Return the profile for the focused Claude harness, if one is focused.
    ///
    /// A worker without an explicit config-dir override inherits the factory
    /// process environment, just like its PTY does. Codex, Grok, shell, and
    /// director panes intentionally have no Claude account badge.
    fn focused_claude_profile(app: &FactoryApp) -> Option<String> {
        let pane = app.mux.focused()?;
        if pane.harness() != SupervisorCli::Claude {
            return None;
        }

        let config_dir = match pane.kind() {
            PaneKind::Worker => {
                let spec = app.mux.effective_worker_spec(pane.id(), None);
                spec.config_dir
                    .or(spec.requester_config_dir)
                    .or_else(|| std::env::var("CLAUDE_CONFIG_DIR").ok())
            }
            PaneKind::Supervisor => std::env::var("CLAUDE_CONFIG_DIR").ok(),
            PaneKind::Director | PaneKind::Shell => return None,
        };

        Some(Self::claude_profile_from_config_dir(config_dir.as_deref()))
    }

    /// Trim leading spans until the rendered width fits.
    /// We drop from the front so the most critical tail hints (e.g., quit key)
    /// remain visible on narrow terminals.
    fn trim_spans_from_front(spans: &mut Vec<Span<'static>>, max_width: usize) {
        if max_width == 0 {
            spans.clear();
            return;
        }

        // Find how many spans to drop from the front so total width fits.
        // Walk forward accumulating widths until removing that prefix brings us under max.
        let total_width: usize = spans.iter().map(Span::width).sum();
        if total_width <= max_width {
            // Already fits, nothing to trim
        } else {
            let excess = total_width - max_width;
            let mut removed_width = 0usize;
            let mut drop_count = 0usize;
            for span in spans.iter() {
                if removed_width >= excess {
                    break;
                }
                removed_width += span.width();
                drop_count += 1;
            }
            spans.drain(..drop_count);
        }

        // Remove stray separators/spaces at the front after trimming.
        let front_trim = spans
            .iter()
            .take_while(|s| Self::is_divider_or_space(s.content.as_ref()))
            .count();
        if front_trim > 0 {
            spans.drain(..front_trim);
        }

        while spans
            .last()
            .map(|s| Self::is_divider_or_space(s.content.as_ref()))
            .unwrap_or(false)
        {
            spans.pop();
        }
    }

    fn spans_display_width(spans: &[Span<'_>]) -> usize {
        spans.iter().map(Span::width).sum()
    }

    fn is_divider_or_space(text: &str) -> bool {
        let trimmed = text.trim();
        trimmed.is_empty() || trimmed == "│"
    }

    /// Add a keyboard shortcut with highlighted key
    fn add_shortcut(
        spans: &mut Vec<Span<'static>>,
        styles: &Styles,
        key: &str,
        suffix: &str,
        key_style: Style,
    ) {
        spans.push(Span::styled(
            format!("[{key}]"),
            key_style.add_modifier(Modifier::BOLD),
        ));
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix.to_string(), styles.text_muted));
        }
    }

    fn update_badge(cas_dir: &Path) -> Option<String> {
        if std::env::var("CAS_DISABLE_UPDATE_CHECK")
            .ok()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        {
            return None;
        }

        Self::refresh_update_cache_async_if_stale(cas_dir);

        let cache_path = cas_dir.join(UPDATE_CHECK_CACHE_RELATIVE);
        let cache = Self::read_update_cache(&cache_path)?;
        if cache.update_available {
            cache
                .latest_version
                .filter(|v| Self::is_newer_version(v, env!("CARGO_PKG_VERSION")))
                .map(|v| format!("⬆ v{v} available"))
        } else {
            None
        }
    }

    fn refresh_update_cache_async_if_stale(cas_dir: &Path) {
        let cache_path = cas_dir.join(UPDATE_CHECK_CACHE_RELATIVE);
        let now = Self::now_unix();
        let existing = Self::read_update_cache(&cache_path);
        let is_fresh = existing.as_ref().is_some_and(|c| {
            let ttl = if c.failed {
                UPDATE_CHECK_CACHE_FAILURE_TTL_SECS
            } else {
                UPDATE_CHECK_CACHE_TTL_SECS
            };
            now.saturating_sub(c.checked_at_unix) < ttl
        });
        if is_fresh {
            return;
        }

        let cas_dir = cas_dir.to_path_buf();
        let mut in_flight = UPDATE_REFRESH_IN_FLIGHT
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .ok();
        if let Some(ref mut set) = in_flight {
            if set.contains(&cas_dir) {
                return;
            }
            set.insert(cas_dir.clone());
        }
        drop(in_flight);

        std::thread::spawn(move || {
            let cache_path = cas_dir.join(UPDATE_CHECK_CACHE_RELATIVE);
            let now = Self::now_unix();
            let previous = Self::read_update_cache(&cache_path);
            let _ = Self::fetch_and_store_update_cache(
                &cache_path,
                env!("CARGO_PKG_VERSION"),
                now,
                previous.as_ref(),
            );

            if let Ok(mut set) = UPDATE_REFRESH_IN_FLIGHT
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
            {
                set.remove(&cas_dir);
            }
        });
    }

    fn read_update_cache(path: &Path) -> Option<UpdateCheckCache> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<UpdateCheckCache>(&raw).ok()
    }

    fn fetch_and_store_update_cache(
        cache_path: &Path,
        current_version: &str,
        checked_at_unix: u64,
        previous: Option<&UpdateCheckCache>,
    ) -> UpdateCheckCache {
        let fetched = Self::fetch_latest_version(current_version).map(|latest| UpdateCheckCache {
            checked_at_unix,
            update_available: Self::is_newer_version(&latest, current_version),
            latest_version: Some(latest),
            failed: false,
        });

        let cache = fetched.unwrap_or_else(|_| {
            let mut fallback = previous.cloned().unwrap_or_default();
            fallback.checked_at_unix = checked_at_unix;
            fallback.failed = true;
            fallback
        });

        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(serialized) = serde_json::to_string(&cache) {
            let _ = std::fs::write(cache_path, serialized);
        }

        cache
    }

    fn fetch_latest_version(current_version: &str) -> anyhow::Result<String> {
        #[derive(Debug, Deserialize)]
        struct ReleaseResponse {
            tag_name: String,
        }

        let response = ureq::AgentBuilder::new()
            .timeout(Duration::from_millis(UPDATE_CHECK_TIMEOUT_MS))
            .build()
            .get(UPDATE_CHECK_URL)
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", &format!("cas/{current_version}"))
            .call()?;

        let release: ReleaseResponse = response.into_json()?;
        Ok(release.tag_name.trim_start_matches('v').to_string())
    }

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn is_newer_version(new: &str, current: &str) -> bool {
        let parse = |v: &str| -> Option<(u32, u32, u32)> {
            let parts: Vec<&str> = v.trim_start_matches('v').split('.').collect();
            if parts.len() >= 3 {
                Some((
                    parts[0].parse().ok()?,
                    parts[1].parse().ok()?,
                    parts[2].split('-').next()?.parse().ok()?,
                ))
            } else {
                None
            }
        };

        match (parse(new), parse(current)) {
            (Some((n1, n2, n3)), Some((c1, c2, c3))) => (n1, n2, n3) > (c1, c2, c3),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StatusBar;
    use cas_mux::{Pane, PaneKind, Pty, PtyConfig, SupervisorCli, WorkerSpec};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;

    fn spawn_claude_worker(name: &str) -> Option<Pane> {
        let pty = Pty::spawn(
            name,
            PtyConfig {
                command: "cat".to_string(),
                args: vec![],
                cwd: None,
                env: vec![],
                rows: 24,
                cols: 80,
            },
        )
        .ok()?;
        Pane::with_pty(name, PaneKind::Worker, pty, 24, 80, SupervisorCli::Claude).ok()
    }

    #[test]
    fn branch_label_drops_before_shortcut_hints_on_narrow_width() {
        assert_eq!(
            StatusBar::branch_label_for_width(Some("factory/fast-octopus-98"), 49),
            None
        );
        assert_eq!(
            StatusBar::branch_label_for_width(Some("factory/fast-octopus-98"), 70),
            Some("branch factory/f…octopus-98".to_string())
        );
    }

    #[test]
    fn branch_label_degrades_on_cache_miss() {
        assert_eq!(StatusBar::branch_label_for_width(None, 120), None);
        assert_eq!(StatusBar::branch_label_for_width(Some(""), 120), None);
    }

    #[test]
    fn claude_profile_uses_main_for_default_and_alt_suffix_for_profile_dir() {
        assert_eq!(StatusBar::claude_profile_from_config_dir(None), "main");
        assert_eq!(
            StatusBar::claude_profile_from_config_dir(Some("~/.claude")),
            "main"
        );
        assert_eq!(
            StatusBar::claude_profile_from_config_dir(Some("/home/test/.claude-alt/")),
            "alt"
        );
    }

    #[test]
    fn render_status_bar_shows_and_emphasizes_focused_worker_alt_profile() {
        let Some(worker_pane) = spawn_claude_worker("alt-worker") else {
            eprintln!("skipping: `cat`/PTY-backed pane unavailable in this environment");
            return;
        };

        let mut app = crate::ui::factory::app::FactoryApp::for_test();
        app.mux.set_worker_spec(
            "alt-worker",
            WorkerSpec {
                name: Some("alt-worker".to_string()),
                cli: SupervisorCli::Claude,
                model: None,
                effort: None,
                config_dir: Some("~/.claude-alt".to_string()),
                requester_config_dir: None,
            },
        );
        app.mux.add_pane(worker_pane);
        assert!(app.mux.focus("alt-worker"), "fixture pane must be focusable");

        let mut terminal = Terminal::new(TestBackend::new(68, 1)).unwrap();
        terminal
            .draw(|frame| StatusBar::render(frame, frame.area(), &app))
            .unwrap();

        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        let account_offset = rendered
            .find("CLAUDE alt")
            .expect("focused Claude worker's alt account must be visible: {rendered}");
        let account_cell = &terminal.backend().buffer()[(account_offset as u16, 0)];
        assert!(
            account_cell.modifier.contains(Modifier::BOLD),
            "alternate account badge must be bold/emphasized"
        );
        assert_ne!(
            account_cell.bg,
            app.theme().palette.bg_elevated,
            "alternate account badge must have a distinct background"
        );
    }
}
