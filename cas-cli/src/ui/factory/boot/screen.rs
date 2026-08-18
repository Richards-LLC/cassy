use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute,
    style::{Attribute, Print, SetAttribute, SetForegroundColor},
    terminal::{Clear, ClearType},
};
use std::io::{Write, stdout};
use std::thread;
use std::time::Duration;

/// Colors for the retro boot screen (CRT aesthetic)
mod colors {
    use crossterm::style::Color;

    // Semantic 4-bit roles let terminal themes control wordmark contrast.
    pub const WORDMARK: Color = Color::Cyan;
    pub const WORDMARK_GLOW: Color = Color::Blue;

    // Existing startup-status glow (the wordmark uses WORDMARK above).
    pub const LOGO_GLOW: Color = Color::Rgb {
        r: 150,
        g: 230,
        b: 255,
    };

    // Text colors
    pub const HEADER: Color = Color::White;
    pub const LABEL: Color = Color::Rgb {
        r: 120,
        g: 120,
        b: 130,
    };
    pub const VALUE: Color = Color::Rgb {
        r: 100,
        g: 200,
        b: 255,
    };

    // Status colors
    pub const OK: Color = Color::Rgb {
        r: 80,
        g: 250,
        b: 120,
    };
    pub const PENDING: Color = Color::Rgb {
        r: 255,
        g: 200,
        b: 80,
    };
    pub const ERROR: Color = Color::Rgb {
        r: 255,
        g: 90,
        b: 90,
    };

    // Progress bar colors
    pub const PROGRESS_DONE: Color = Color::Rgb {
        r: 80,
        g: 220,
        b: 120,
    };
    pub const PROGRESS_EMPTY: Color = Color::Rgb {
        r: 50,
        g: 50,
        b: 55,
    };

    // Agent role colors
    pub const WORKER: Color = Color::Rgb {
        r: 180,
        g: 130,
        b: 255,
    };
    pub const SUPERVISOR: Color = Color::Rgb {
        r: 255,
        g: 180,
        b: 80,
    };

    // Box/frame colors
    pub const BOX: Color = Color::Rgb {
        r: 70,
        g: 70,
        b: 75,
    };

    // Final ready state
    pub const READY: Color = Color::Rgb {
        r: 100,
        g: 255,
        b: 180,
    };
}

/// Minions-themed colors for the boot screen
mod minions_colors {
    use crossterm::style::Color;

    // The alternate palette preserves Cassy's identity while respecting the
    // terminal's standard ANSI-color theme.
    pub const WORDMARK: Color = Color::Yellow;
    pub const WORDMARK_GLOW: Color = Color::DarkYellow;

    // Existing startup-status glow (the wordmark uses WORDMARK above).
    pub const LOGO_GLOW: Color = Color::Rgb {
        r: 255,
        g: 235,
        b: 100,
    };

    // Text colors
    pub const HEADER: Color = Color::White;
    pub const LABEL: Color = Color::Rgb {
        r: 120,
        g: 120,
        b: 130,
    };
    pub const VALUE: Color = Color::Rgb {
        r: 255,
        g: 213,
        b: 0,
    };

    // Status colors (keep functional)
    pub const OK: Color = Color::Rgb {
        r: 80,
        g: 250,
        b: 120,
    };
    pub const PENDING: Color = Color::Rgb {
        r: 255,
        g: 213,
        b: 0,
    };
    pub const ERROR: Color = Color::Rgb {
        r: 255,
        g: 90,
        b: 90,
    };

    // Progress bar - yellow fill
    pub const PROGRESS_DONE: Color = Color::Rgb {
        r: 255,
        g: 213,
        b: 0,
    };
    pub const PROGRESS_EMPTY: Color = Color::Rgb {
        r: 50,
        g: 50,
        b: 55,
    };

    // Agent role colors - denim blue for workers, dark for supervisor (Gru)
    pub const WORKER: Color = Color::Rgb {
        r: 65,
        g: 105,
        b: 225,
    };
    pub const SUPERVISOR: Color = Color::Rgb {
        r: 80,
        g: 80,
        b: 85,
    };

    // Box/frame colors - denim blue tint
    pub const BOX: Color = Color::Rgb {
        r: 50,
        g: 60,
        b: 90,
    };

    // Final ready state - banana yellow
    pub const READY: Color = Color::Rgb {
        r: 255,
        g: 235,
        b: 59,
    };
}

/// Compact six-row Cassy wordmark. Its 49-cell widest row fits an 80-column
/// terminal and keeps the boot screen within the normal 24-row viewport.
const LOGO: &str = r#"
 ██████╗ █████╗ ███████╗███████╗██╗   ██╗
██╔════╝██╔══██╗██╔════╝██╔════╝╚██╗ ██╔╝
██║     ███████║███████╗███████╗ ╚████╔╝
██║     ██╔══██║╚════██║╚════██║  ╚██╔╝
╚██████╗██║  ██║███████║███████║   ██║
 ╚═════╝╚═╝  ╚═╝╚══════╝╚══════╝   ╚═╝
"#;

/// Same Cassy wordmark for narrow terminals; the full design is already safe
/// at 80 columns, so no abbreviated mark is needed.
const LOGO_SMALL: &str = r#"
 ██████╗ █████╗ ███████╗███████╗██╗   ██╗
██╔════╝██╔══██╗██╔════╝██╔════╝╚██╗ ██╔╝
██║     ███████║███████╗███████╗ ╚████╔╝
██║     ██╔══██║╚════██║╚════██║  ╚██╔╝
╚██████╗██║  ██║███████║███████║   ██║
 ╚═════╝╚═╝  ╚═╝╚══════╝╚══════╝   ╚═╝
"#;

/// The Minions palette still renders the Cassy wordmark; a theme must not
/// change the product identity at startup.
const MINION_LOGO: &str = r#"
 ██████╗ █████╗ ███████╗███████╗██╗   ██╗
██╔════╝██╔══██╗██╔════╝██╔════╝╚██╗ ██╔╝
██║     ███████║███████╗███████╗ ╚████╔╝
██║     ██╔══██║╚════██║╚════██║  ╚██╔╝
╚██████╗██║  ██║███████║███████║   ██║
 ╚═════╝╚═╝  ╚═╝╚══════╝╚══════╝   ╚═╝
"#;

/// Narrow Minions boot uses the same compact Cassy wordmark.
const MINION_LOGO_SMALL: &str = r#"
 ██████╗ █████╗ ███████╗███████╗██╗   ██╗
██╔════╝██╔══██╗██╔════╝██╔════╝╚██╗ ██╔╝
██║     ███████║███████╗███████╗ ╚████╔╝
██║     ██╔══██║╚════██║╚════██║  ╚██╔╝
╚██████╗██║  ██║███████║███████║   ██║
 ╚═════╝╚═╝  ╚═╝╚══════╝╚══════╝   ╚═╝
"#;

/// Braille spinner frames for smooth animation
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Progress bar gradient characters (smooth fill)
const PROGRESS_CHARS: &[char] = &['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Boot screen state for rendering
pub(crate) struct BootScreen {
    pub(crate) stdout: std::io::Stdout,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) box_left: u16,
    pub(crate) box_width: u16,
    pub(crate) steps_row: u16,
    pub(crate) agent_row: u16,
    pub(crate) skip_animation: bool,
    pub(crate) minions_theme: bool,
    spinner_tick: usize,
}

/// Helper to select default or minions color
macro_rules! themed {
    ($self:expr, $name:ident) => {
        if $self.minions_theme {
            minions_colors::$name
        } else {
            colors::$name
        }
    };
}

impl BootScreen {
    pub(crate) fn new_themed(skip_animation: bool, minions_theme: bool) -> std::io::Result<Self> {
        let mut stdout = stdout();
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));

        execute!(stdout, Clear(ClearType::All), Hide, MoveTo(0, 0))?;

        let box_width = 70.min(cols - 4);
        let box_left = (cols - box_width) / 2;

        Ok(Self {
            stdout,
            cols,
            rows,
            box_left,
            box_width,
            steps_row: 0, // Set after logo
            agent_row: 0, // Set after steps
            skip_animation,
            minions_theme,
            spinner_tick: 0,
        })
    }
    pub(crate) fn draw_logo(&mut self) -> std::io::Result<u16> {
        let logo_color = themed!(self, WORDMARK);
        let logo_glow = themed!(self, WORDMARK_GLOW);
        let header_color = themed!(self, HEADER);
        let label_color = themed!(self, LABEL);

        let (title, subtitle) = if self.minions_theme {
            (
                "═══  BANANA!  ═══",
                format!("Bee-do Bee-do  •  v{}", APP_VERSION),
            )
        } else {
            (
                "═══  Cassy Coding Agent System  ═══",
                format!("Multi-Agent Orchestration  •  v{}", APP_VERSION),
            )
        };

        let compact_title = if self.minions_theme {
            "Minion Factory Boot"
        } else {
            "Cassy Factory Boot"
        };

        let compact_subtitle = if self.minions_theme {
            format!("Bee-do Bee-do  •  v{}", APP_VERSION)
        } else {
            format!("Coding Agent System  •  v{}", APP_VERSION)
        };

        // Tmux and many terminal defaults are 24 rows tall. The full logo + subtitle
        // pushes the boot box out of view, so fall back to a compact header.
        if self.rows < 36 {
            execute!(
                self.stdout,
                MoveTo(0, 1),
                SetForegroundColor(header_color),
                SetAttribute(Attribute::Bold),
                Print(format!(
                    "{:^width$}",
                    compact_title,
                    width = self.cols as usize
                )),
                SetAttribute(Attribute::Reset),
                MoveTo(0, 2),
                SetForegroundColor(label_color),
                Print(format!(
                    "{:^width$}",
                    compact_subtitle,
                    width = self.cols as usize
                )),
            )?;
            self.stdout.flush()?;
            if !self.skip_animation {
                thread::sleep(Duration::from_millis(120));
            }
            return Ok(4);
        }

        let delay = if self.skip_animation { 0 } else { 35 };
        let logo = if self.minions_theme {
            if self.cols >= 100 { MINION_LOGO } else { MINION_LOGO_SMALL }
        } else {
            if self.cols >= 100 { LOGO } else { LOGO_SMALL }
        };
        let logo_lines: Vec<&str> = logo.lines().filter(|l| !l.is_empty()).collect();

        // Starting row with top padding
        let logo_start_row = 2u16;

        for (i, line) in logo_lines.iter().enumerate() {
            let padding = ((self.cols as usize).saturating_sub(line.chars().count())) / 2;
            let row = logo_start_row + i as u16;

            if !self.skip_animation {
                // Bright glow effect (more visible)
                execute!(
                    self.stdout,
                    MoveTo(padding as u16, row),
                    SetForegroundColor(logo_glow),
                    SetAttribute(Attribute::Bold),
                    Print(line),
                    SetAttribute(Attribute::Reset)
                )?;
                self.stdout.flush()?;
                thread::sleep(Duration::from_millis(delay));

                // Fade to normal color
                execute!(
                    self.stdout,
                    MoveTo(padding as u16, row),
                    SetForegroundColor(logo_color),
                    Print(line)
                )?;
                self.stdout.flush()?;
                thread::sleep(Duration::from_millis(delay / 2));
            } else {
                // No animation - just draw
                execute!(
                    self.stdout,
                    MoveTo(padding as u16, row),
                    SetForegroundColor(logo_color),
                    Print(line)
                )?;
            }
        }

        // Add extra spacing after logo
        let subtitle_row = logo_start_row + logo_lines.len() as u16 + 2;

        // Version and subtitle with better styling
        execute!(
            self.stdout,
            MoveTo(0, subtitle_row),
            SetForegroundColor(header_color),
            SetAttribute(Attribute::Bold),
            Print(format!(
                "{:^width$}",
                title,
                width = self.cols as usize
            )),
            SetAttribute(Attribute::Reset),
            MoveTo(0, subtitle_row + 1),
            SetForegroundColor(label_color),
            Print(format!(
                "{:^width$}",
                subtitle,
                width = self.cols as usize
            )),
        )?;
        self.stdout.flush()?;

        if !self.skip_animation {
            thread::sleep(Duration::from_millis(300));
        }

        // Return row for box start (with extra spacing)
        Ok(subtitle_row + 4)
    }
    pub(crate) fn draw_box(
        &mut self,
        session_name: &str,
        cwd: &str,
        profile: &str,
        num_workers: usize,
    ) -> std::io::Result<()> {
        // Calculate box height with better spacing:
        // - 3 rows for session/context info
        // - 1 separator
        // - 1 section header for initialization
        // - up to 6 initialization steps
        // - 1 separator
        // - 1 section header for agents
        // - 1 supervisor + workers
        // - 1 ready message
        // - 2 padding rows
        let num_agents = 1 + num_workers;
        let box_height = 3 + 1 + 1 + 7 + 1 + 1 + num_agents as u16 + 2 + 2;

        // On short terminals (e.g. tmux defaults), shift the box up so the
        // full initialization and agent sections stay inside the viewport.
        let max_row = self.rows.saturating_sub(1);
        let mut steps_row = self.steps_row.max(1);
        if steps_row + box_height > max_row {
            let overflow = steps_row + box_height - max_row;
            steps_row = steps_row.saturating_sub(overflow).max(1);
        }
        self.steps_row = steps_row;

        // Draw box outline
        execute!(self.stdout, SetForegroundColor(themed!(self, BOX)))?;

        // Top border with double line for emphasis
        execute!(
            self.stdout,
            MoveTo(self.box_left, self.steps_row - 1),
            Print("╭"),
            Print("─".repeat((self.box_width - 2) as usize)),
            Print("╮")
        )?;

        // Sides
        for row in 0..box_height {
            execute!(
                self.stdout,
                MoveTo(self.box_left, self.steps_row + row),
                Print("│"),
                MoveTo(self.box_left + self.box_width - 1, self.steps_row + row),
                Print("│")
            )?;
        }

        // Bottom border
        execute!(
            self.stdout,
            MoveTo(self.box_left, self.steps_row + box_height),
            Print("╰"),
            Print("─".repeat((self.box_width - 2) as usize)),
            Print("╯")
        )?;

        // Session info with better spacing
        self.print_labeled(self.steps_row + 1, "Session", session_name)?;
        self.print_labeled(
            self.steps_row + 2,
            "Directory",
            &truncate_path(cwd, (self.box_width - 18) as usize),
        )?;
        self.print_labeled(
            self.steps_row + 3,
            "Profile",
            &truncate_path(profile, (self.box_width - 18) as usize),
        )?;

        // Separator with section label
        self.draw_section_divider(self.steps_row + 5, "INITIALIZATION")?;

        self.stdout.flush()?;
        Ok(())
    }
    pub(crate) fn draw_section_divider(&mut self, row: u16, label: &str) -> std::io::Result<()> {
        let label_with_padding = format!(" {label} ");
        let label_len = label_with_padding.chars().count();
        let side_len = ((self.box_width - 2) as usize - label_len) / 2;
        let right_side = (self.box_width - 2) as usize - label_len - side_len;

        execute!(
            self.stdout,
            MoveTo(self.box_left + 1, row),
            SetForegroundColor(themed!(self, BOX)),
            Print("─".repeat(side_len)),
            SetForegroundColor(themed!(self, LABEL)),
            Print(&label_with_padding),
            SetForegroundColor(themed!(self, BOX)),
            Print("─".repeat(right_side))
        )?;
        Ok(())
    }
    pub(crate) fn print_labeled(
        &mut self,
        row: u16,
        label: &str,
        value: &str,
    ) -> std::io::Result<()> {
        execute!(
            self.stdout,
            MoveTo(self.box_left + 2, row),
            SetForegroundColor(themed!(self, LABEL)),
            Print(format!("{label:>12}: ")),
            SetForegroundColor(themed!(self, VALUE)),
            Print(value)
        )?;
        Ok(())
    }
    pub(crate) fn start_step(&mut self, row: u16, text: &str) -> std::io::Result<()> {
        execute!(
            self.stdout,
            MoveTo(self.box_left + 4, row),
            SetForegroundColor(themed!(self, PENDING)),
            Print(SPINNER_FRAMES[0]),
            Print("  "),
            SetForegroundColor(themed!(self, HEADER)),
            Print(text),
            SetForegroundColor(themed!(self, LABEL)),
            Print(" ...")
        )?;
        self.stdout.flush()?;
        Ok(())
    }
    pub(crate) fn spin_step(&mut self, row: u16, iterations: u64) -> std::io::Result<()> {
        if self.skip_animation {
            return Ok(());
        }

        for i in 0..iterations {
            let frame_idx = (self.spinner_tick + i as usize) % SPINNER_FRAMES.len();
            execute!(
                self.stdout,
                MoveTo(self.box_left + 4, row),
                SetForegroundColor(themed!(self, PENDING)),
                Print(SPINNER_FRAMES[frame_idx])
            )?;
            self.stdout.flush()?;
            thread::sleep(Duration::from_millis(60));
        }
        self.spinner_tick = (self.spinner_tick + iterations as usize) % SPINNER_FRAMES.len();
        Ok(())
    }
    pub(crate) fn complete_step(&mut self, row: u16, text: &str) -> std::io::Result<()> {
        execute!(
            self.stdout,
            MoveTo(self.box_left + 4, row),
            SetForegroundColor(themed!(self, OK)),
            Print("✓"),
            Print("  "),
            SetForegroundColor(themed!(self, HEADER)),
            Print(text),
            Print("        ") // Clear any remnants
        )?;
        self.stdout.flush()?;
        Ok(())
    }
    pub(crate) fn fail_step(&mut self, row: u16, text: &str, error: &str) -> std::io::Result<()> {
        execute!(
            self.stdout,
            MoveTo(self.box_left + 4, row),
            SetForegroundColor(themed!(self, ERROR)),
            Print("✗"),
            Print("  "),
            SetForegroundColor(themed!(self, HEADER)),
            Print(text),
            SetForegroundColor(themed!(self, LABEL)),
            Print(" — "),
            SetForegroundColor(themed!(self, ERROR)),
            Print(truncate_path(error, 30))
        )?;
        self.stdout.flush()?;
        Ok(())
    }
    pub(crate) fn start_agent(
        &mut self,
        row: u16,
        name: &str,
        is_supervisor: bool,
    ) -> std::io::Result<()> {
        let role = if is_supervisor {
            "supervisor"
        } else {
            "worker"
        };
        let role_color = if is_supervisor {
            themed!(self, SUPERVISOR)
        } else {
            themed!(self, WORKER)
        };
        let bar_width = 24;
        let name_width = 14;

        execute!(
            self.stdout,
            MoveTo(self.box_left + 4, row),
            SetForegroundColor(role_color),
            Print(format!("{role:>10}")),
            Print("  "),
            SetForegroundColor(themed!(self, VALUE)),
            Print(format!("{name:<name_width$}")),
            Print("  "),
            SetForegroundColor(themed!(self, BOX)),
            Print("▐"),
            SetForegroundColor(themed!(self, PROGRESS_EMPTY)),
            Print("░".repeat(bar_width)),
            SetForegroundColor(themed!(self, BOX)),
            Print("▌"),
            Print(" "),
            SetForegroundColor(themed!(self, PENDING)),
            Print("INIT")
        )?;
        self.stdout.flush()?;
        Ok(())
    }
    pub(crate) fn update_agent_progress(&mut self, row: u16, progress: f32) -> std::io::Result<()> {
        let bar_width = 24;
        let name_width = 14;

        // Calculate filled portion with sub-character precision
        let total_units = bar_width * 8; // 8 sub-units per character
        let filled_units = ((progress * total_units as f32) as usize).min(total_units);
        let full_chars = filled_units / 8;
        let partial_char_idx = filled_units % 8;
        let empty_chars = bar_width - full_chars - if partial_char_idx > 0 { 1 } else { 0 };

        let done_color = themed!(self, PROGRESS_DONE);
        let empty_color = themed!(self, PROGRESS_EMPTY);

        // Move to progress bar position
        execute!(
            self.stdout,
            MoveTo(self.box_left + 4 + 12 + name_width as u16 + 3, row),
            SetForegroundColor(done_color),
            Print("█".repeat(full_chars))
        )?;

        // Draw partial character if needed
        if partial_char_idx > 0 {
            execute!(
                self.stdout,
                SetForegroundColor(done_color),
                Print(PROGRESS_CHARS[partial_char_idx - 1])
            )?;
        }

        // Draw empty portion
        execute!(
            self.stdout,
            SetForegroundColor(empty_color),
            Print("░".repeat(empty_chars))
        )?;

        self.stdout.flush()?;
        Ok(())
    }
    pub(crate) fn complete_agent(&mut self, row: u16) -> std::io::Result<()> {
        let bar_width = 24;
        let name_width = 14;

        execute!(
            self.stdout,
            MoveTo(self.box_left + 4 + 12 + name_width as u16 + 3, row),
            SetForegroundColor(themed!(self, PROGRESS_DONE)),
            Print("█".repeat(bar_width)),
            SetForegroundColor(themed!(self, BOX)),
            Print("▌"),
            Print(" "),
            SetForegroundColor(themed!(self, OK)),
            SetAttribute(Attribute::Bold),
            Print("READY"),
            SetAttribute(Attribute::Reset)
        )?;
        self.stdout.flush()?;
        Ok(())
    }
    pub(crate) fn show_ready(&mut self, final_row: u16) -> std::io::Result<()> {
        let ready_color = themed!(self, READY);
        let glow_color = themed!(self, LOGO_GLOW);
        let label_color = themed!(self, LABEL);

        let (ready_text, launch_text) = if self.minions_theme {
            ("  BANANA!", "  —  Bee-do Bee-do Bee-do")
        } else {
            ("  SYSTEM READY", "  —  Launching interface")
        };

        if !self.skip_animation {
            // Pulsing animation before showing ready
            for _ in 0..3 {
                execute!(
                    self.stdout,
                    MoveTo(self.box_left + 4, final_row),
                    SetForegroundColor(glow_color),
                    SetAttribute(Attribute::Bold),
                    Print("●"),
                    SetAttribute(Attribute::Reset),
                )?;
                self.stdout.flush()?;
                thread::sleep(Duration::from_millis(100));

                execute!(
                    self.stdout,
                    MoveTo(self.box_left + 4, final_row),
                    SetForegroundColor(ready_color),
                    Print("○"),
                )?;
                self.stdout.flush()?;
                thread::sleep(Duration::from_millis(100));
            }
        }

        // Final ready message
        execute!(
            self.stdout,
            MoveTo(self.box_left + 4, final_row),
            SetForegroundColor(ready_color),
            SetAttribute(Attribute::Bold),
            Print("▶"),
            Print(ready_text),
            SetAttribute(Attribute::Reset),
        )?;
        self.stdout.flush()?;

        if !self.skip_animation {
            thread::sleep(Duration::from_millis(200));

            let ready_len = ready_text.len() as u16 + 1; // +1 for ▶
            // Type out the launching message
            for (i, ch) in launch_text.chars().enumerate() {
                execute!(
                    self.stdout,
                    MoveTo(self.box_left + 4 + ready_len + i as u16, final_row),
                    SetForegroundColor(label_color),
                    Print(ch)
                )?;
                self.stdout.flush()?;
                thread::sleep(Duration::from_millis(25));
            }

            // Brief pause with dots animation
            for _ in 0..3 {
                execute!(self.stdout, Print("."))?;
                self.stdout.flush()?;
                thread::sleep(Duration::from_millis(150));
            }

            thread::sleep(Duration::from_millis(300));
        }

        Ok(())
    }
    pub(crate) fn cleanup(&mut self) -> std::io::Result<()> {
        execute!(self.stdout, Show, Clear(ClearType::All))?;
        Ok(())
    }
}

/// Truncate a path for display (UTF-8 safe, keeps suffix)
fn truncate_path(path: &str, max_len: usize) -> String {
    let char_count = path.chars().count();
    if char_count <= max_len {
        path.to_string()
    } else if max_len <= 5 {
        "...".to_string()
    } else {
        let suffix_len = max_len - 3;
        let suffix: String = path.chars().skip(char_count - suffix_len).collect();
        format!("...{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::{LOGO, LOGO_SMALL, MINION_LOGO, MINION_LOGO_SMALL};

    #[test]
    fn cassy_boot_wordmarks_fit_at_80_columns() {
        for logo in [LOGO, LOGO_SMALL, MINION_LOGO, MINION_LOGO_SMALL] {
            let rows: Vec<_> = logo.lines().filter(|row| !row.is_empty()).collect();
            assert_eq!(rows.len(), 6, "the boot splash must remain compact");
            assert!(rows.iter().all(|row| row.chars().count() <= 80));
        }
    }
}
