//! Formatter — styled output abstraction for CLI rendering
//!
//! The Formatter is the core output primitive for all Cassy CLI display. It:
//! - Auto-detects TTY vs piped output
//! - Respects the NO_COLOR environment variable
//! - Queries terminal width via crossterm
//! - Routes through ActiveTheme for consistent styling

use std::io::{self, IsTerminal, Write};

use crossterm::Command;
use crossterm::style::{
    Attribute, Color as CrosstermColor, ResetColor, SetAttribute, SetForegroundColor,
};
use ratatui::style::Color as RatatuiColor;

use crate::ui::theme::{ActiveTheme, Icons};

/// Output mode controlling style behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Full color and styling (TTY, no NO_COLOR)
    Styled,
    /// Plain text only (piped, NO_COLOR set, or explicitly requested)
    Plain,
}

impl OutputMode {
    /// Detect the appropriate output mode for stdout.
    pub fn detect() -> Self {
        if std::env::var("NO_COLOR").is_ok() {
            return OutputMode::Plain;
        }
        if io::stdout().is_terminal() {
            OutputMode::Styled
        } else {
            OutputMode::Plain
        }
    }
}

/// Write a crossterm Command as ANSI bytes to any `dyn Write`.
///
/// This avoids the `crossterm::execute!` macro which requires `Sized`.
pub fn emit(w: &mut dyn Write, cmd: &impl Command) -> io::Result<()> {
    let mut ansi = String::new();
    cmd.write_ansi(&mut ansi).map_err(io::Error::other)?;
    w.write_all(ansi.as_bytes())
}

/// Styled output formatter backed by a theme and terminal detection.
///
/// The Formatter writes directly to a `Write` sink — typically stdout.
/// In styled mode it emits crossterm escape sequences; in plain mode it
/// emits raw text with no ANSI codes.
pub struct Formatter<'w> {
    writer: &'w mut dyn Write,
    mode: OutputMode,
    theme: ActiveTheme,
    width: u16,
}

impl<'w> Formatter<'w> {
    /// Create a Formatter for stdout with auto-detected mode and theme.
    pub fn stdout(writer: &'w mut dyn Write, theme: ActiveTheme) -> Self {
        let mode = OutputMode::detect();
        let width = terminal_width();
        Self {
            writer,
            mode,
            theme,
            width,
        }
    }

    /// Create a styled Formatter (force styled mode).
    pub fn styled(writer: &'w mut dyn Write, theme: ActiveTheme) -> Self {
        let width = terminal_width();
        Self {
            writer,
            mode: OutputMode::Styled,
            theme,
            width,
        }
    }

    /// Create a plain-text Formatter (no ANSI codes, default theme).
    pub fn plain(writer: &'w mut dyn Write) -> Self {
        Self {
            writer,
            mode: OutputMode::Plain,
            theme: ActiveTheme::default(),
            width: 80,
        }
    }

    /// Create a Formatter with explicit configuration.
    pub fn new(
        writer: &'w mut dyn Write,
        mode: OutputMode,
        theme: ActiveTheme,
        width: u16,
    ) -> Self {
        Self {
            writer,
            mode,
            theme,
            width,
        }
    }

    // ========================================================================
    // Accessors
    // ========================================================================

    /// Current output mode.
    pub fn mode(&self) -> OutputMode {
        self.mode
    }

    /// Whether output is styled (TTY with color support).
    pub fn is_styled(&self) -> bool {
        self.mode == OutputMode::Styled
    }

    /// Terminal width in columns.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Reference to the active theme.
    pub fn theme(&self) -> &ActiveTheme {
        &self.theme
    }

    // ========================================================================
    // Primitive write helpers
    // ========================================================================

    /// Write text with a specific ratatui color.
    pub fn write_colored(&mut self, text: &str, color: RatatuiColor) -> io::Result<()> {
        if self.mode == OutputMode::Styled {
            emit(self.writer, &SetForegroundColor(to_crossterm_color(color)))?;
            self.writer.write_all(text.as_bytes())?;
            emit(self.writer, &ResetColor)
        } else {
            write!(self.writer, "{text}")
        }
    }

    /// Write text with a crossterm color directly.
    pub fn write_crossterm_colored(&mut self, text: &str, color: CrosstermColor) -> io::Result<()> {
        if self.mode == OutputMode::Styled {
            emit(self.writer, &SetForegroundColor(color))?;
            self.writer.write_all(text.as_bytes())?;
            emit(self.writer, &ResetColor)
        } else {
            write!(self.writer, "{text}")
        }
    }

    /// Write bold text with a specific color.
    pub fn write_bold_colored(&mut self, text: &str, color: RatatuiColor) -> io::Result<()> {
        if self.mode == OutputMode::Styled {
            emit(self.writer, &SetForegroundColor(to_crossterm_color(color)))?;
            emit(self.writer, &SetAttribute(Attribute::Bold))?;
            self.writer.write_all(text.as_bytes())?;
            emit(self.writer, &SetAttribute(Attribute::Reset))
        } else {
            write!(self.writer, "{text}")
        }
    }

    /// Write text with the primary text color.
    pub fn write_primary(&mut self, text: &str) -> io::Result<()> {
        let color = self.theme.palette.text_primary;
        self.write_colored(text, color)
    }

    /// Write text with the secondary text color.
    pub fn write_secondary(&mut self, text: &str) -> io::Result<()> {
        let color = self.theme.palette.text_secondary;
        self.write_colored(text, color)
    }

    /// Write text with the muted text color.
    pub fn write_muted(&mut self, text: &str) -> io::Result<()> {
        let color = self.theme.palette.text_muted;
        self.write_colored(text, color)
    }

    /// Write text with the accent color.
    pub fn write_accent(&mut self, text: &str) -> io::Result<()> {
        let color = self.theme.palette.accent;
        self.write_colored(text, color)
    }

    /// Write bold text with the primary color.
    pub fn write_bold(&mut self, text: &str) -> io::Result<()> {
        let color = self.theme.palette.text_primary;
        self.write_bold_colored(text, color)
    }

    /// Write plain text with no styling.
    pub fn write_raw(&mut self, text: &str) -> io::Result<()> {
        write!(self.writer, "{text}")
    }

    /// Write a newline.
    pub fn newline(&mut self) -> io::Result<()> {
        writeln!(self.writer)
    }

    /// Flush the underlying writer.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    // ========================================================================
    // Semantic convenience methods
    // ========================================================================

    /// Print a section heading with accent color and bold.
    ///
    /// Output: `═══ HEADING ═══` (styled) or `--- HEADING ---` (plain)
    pub fn heading(&mut self, text: &str) -> io::Result<()> {
        if self.mode == OutputMode::Styled {
            let bar = Icons::SEPARATOR_DOUBLE;
            let color = self.theme.palette.accent;
            self.write_bold_colored(&format!("{bar}{bar}{bar} {text} {bar}{bar}{bar}"), color)?;
        } else {
            write!(self.writer, "--- {text} ---")?;
        }
        self.newline()
    }

    /// Print a sub-heading with the primary text color and bold.
    pub fn subheading(&mut self, text: &str) -> io::Result<()> {
        self.write_bold(text)?;
        self.newline()
    }

    /// Print a status line: `label  value`
    ///
    /// The label is secondary color; the value is primary text.
    pub fn status(&mut self, label: &str, value: &str) -> io::Result<()> {
        let color = self.theme.palette.text_secondary;
        self.write_colored(label, color)?;
        self.write_raw("  ")?;
        self.write_primary(value)?;
        self.newline()
    }

    /// Print a success status: `✓ message`
    pub fn success(&mut self, message: &str) -> io::Result<()> {
        let color = self.theme.palette.status_success;
        if self.mode == OutputMode::Styled {
            self.write_colored(&format!("{} {message}", Icons::CHECK), color)?;
        } else {
            write!(self.writer, "[OK] {message}")?;
        }
        self.newline()
    }

    /// Print a warning: `⚠ message`
    pub fn warning(&mut self, message: &str) -> io::Result<()> {
        let color = self.theme.palette.status_warning;
        if self.mode == OutputMode::Styled {
            self.write_colored(&format!("{} {message}", Icons::WARNING), color)?;
        } else {
            write!(self.writer, "[WARN] {message}")?;
        }
        self.newline()
    }

    /// Print an error: `✗ message`
    pub fn error(&mut self, message: &str) -> io::Result<()> {
        let color = self.theme.palette.status_error;
        if self.mode == OutputMode::Styled {
            self.write_colored(&format!("{} {message}", Icons::CROSS), color)?;
        } else {
            write!(self.writer, "[ERROR] {message}")?;
        }
        self.newline()
    }

    /// Print an info message: `ℹ message`
    pub fn info(&mut self, message: &str) -> io::Result<()> {
        let color = self.theme.palette.status_info;
        if self.mode == OutputMode::Styled {
            self.write_colored(&format!("{} {message}", Icons::INFO), color)?;
        } else {
            write!(self.writer, "[INFO] {message}")?;
        }
        self.newline()
    }

    /// Print a labeled field: `label: value`
    pub fn field(&mut self, label: &str, value: &str) -> io::Result<()> {
        let label_color = self.theme.palette.text_secondary;
        self.write_colored(label, label_color)?;
        self.write_raw(": ")?;
        self.write_primary(value)?;
        self.newline()
    }

    /// Print a labeled field with the value in accent color.
    pub fn field_accent(&mut self, label: &str, value: &str) -> io::Result<()> {
        let label_color = self.theme.palette.text_secondary;
        self.write_colored(label, label_color)?;
        self.write_raw(": ")?;
        self.write_accent(value)?;
        self.newline()
    }

    /// Print a bulleted list item: `• text`
    pub fn bullet(&mut self, text: &str) -> io::Result<()> {
        if self.mode == OutputMode::Styled {
            let color = self.theme.palette.text_muted;
            self.write_colored(&format!("  {} ", Icons::BULLET), color)?;
            self.write_primary(text)?;
        } else {
            write!(self.writer, "  - {text}")?;
        }
        self.newline()
    }

    /// Print a horizontal separator spanning the terminal width.
    pub fn separator(&mut self) -> io::Result<()> {
        if self.mode == OutputMode::Styled {
            let color = self.theme.palette.border_muted;
            let line = Icons::SEPARATOR.repeat(self.width as usize);
            self.write_colored(&line, color)?;
        } else {
            let line = "-".repeat(self.width as usize);
            write!(self.writer, "{line}")?;
        }
        self.newline()
    }

    /// Print an indented block of text with a vertical bar prefix.
    pub fn indent_block(&mut self, text: &str) -> io::Result<()> {
        let border_color = self.theme.palette.border_muted;
        for line in text.lines() {
            if self.mode == OutputMode::Styled {
                self.write_colored(&format!("  {} ", Icons::VERTICAL_LINE), border_color)?;
            } else {
                write!(self.writer, "  | ")?;
            }
            self.write_primary(line)?;
            self.newline()?;
        }
        Ok(())
    }

    /// Print a key-hint: `[key] description`
    pub fn key_hint(&mut self, key: &str, description: &str) -> io::Result<()> {
        if self.mode == OutputMode::Styled {
            let key_color = self.theme.palette.hint_key;
            let desc_color = self.theme.palette.hint_description;
            self.write_colored(&format!("[{key}]"), key_color)?;
            self.write_raw(" ")?;
            self.write_colored(description, desc_color)?;
        } else {
            write!(self.writer, "[{key}] {description}")?;
        }
        self.newline()
    }

    /// Print a progress indicator: `[████░░░░] 50%`
    pub fn progress(&mut self, current: usize, total: usize) -> io::Result<()> {
        if total == 0 {
            return self.newline();
        }

        let bar_width = 20usize;
        let filled = (current * bar_width) / total;
        let empty = bar_width - filled;
        let pct = (current * 100) / total;

        if self.mode == OutputMode::Styled {
            let filled_color = self.theme.palette.accent;
            let empty_color = self.theme.palette.border_muted;

            self.write_raw("[")?;
            let filled_str = Icons::PROGRESS_FULL.repeat(filled);
            self.write_colored(&filled_str, filled_color)?;
            let empty_str = Icons::PROGRESS_EMPTY.repeat(empty);
            self.write_colored(&empty_str, empty_color)?;
            self.write_raw("] ")?;
            self.write_primary(&format!("{pct}%"))?;
        } else {
            let filled_str = "#".repeat(filled);
            let empty_str = ".".repeat(empty);
            write!(self.writer, "[{filled_str}{empty_str}] {pct}%")?;
        }
        self.newline()
    }
}

// ============================================================================
// Utility functions
// ============================================================================

/// Query terminal width, defaulting to 80 columns.
pub fn terminal_width() -> u16 {
    crossterm::terminal::size()
        .map(|(cols, _)| cols)
        .unwrap_or(80)
}

/// Convert a ratatui Color to a crossterm Color.
///
/// Both use identical RGB representation under the hood, but they are
/// separate enum types.
pub fn to_crossterm_color(color: RatatuiColor) -> CrosstermColor {
    match color {
        RatatuiColor::Reset => CrosstermColor::Reset,
        RatatuiColor::Black => CrosstermColor::Black,
        RatatuiColor::Red => CrosstermColor::DarkRed,
        RatatuiColor::Green => CrosstermColor::DarkGreen,
        RatatuiColor::Yellow => CrosstermColor::DarkYellow,
        RatatuiColor::Blue => CrosstermColor::DarkBlue,
        RatatuiColor::Magenta => CrosstermColor::DarkMagenta,
        RatatuiColor::Cyan => CrosstermColor::DarkCyan,
        RatatuiColor::Gray => CrosstermColor::Grey,
        RatatuiColor::DarkGray => CrosstermColor::DarkGrey,
        RatatuiColor::LightRed => CrosstermColor::Red,
        RatatuiColor::LightGreen => CrosstermColor::Green,
        RatatuiColor::LightYellow => CrosstermColor::Yellow,
        RatatuiColor::LightBlue => CrosstermColor::Blue,
        RatatuiColor::LightMagenta => CrosstermColor::Magenta,
        RatatuiColor::LightCyan => CrosstermColor::Cyan,
        RatatuiColor::White => CrosstermColor::White,
        RatatuiColor::Rgb(r, g, b) => CrosstermColor::Rgb { r, g, b },
        RatatuiColor::Indexed(idx) => CrosstermColor::AnsiValue(idx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_mode_detect_respects_no_color() {
        let mode = OutputMode::detect();
        // In test (non-TTY), should be Plain
        assert_eq!(mode, OutputMode::Plain);
    }

    #[test]
    fn test_plain_formatter_no_ansi() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.write_primary("hello").unwrap();
            fmt.newline().unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "hello\n");
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn test_styled_formatter_emits_ansi() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            fmt.write_primary("styled").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\x1b["));
        assert!(output.contains("styled"));
    }

    #[test]
    fn test_heading_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.heading("Tasks").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "--- Tasks ---\n");
    }

    #[test]
    fn test_heading_styled() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            fmt.heading("Tasks").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Tasks"));
        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_success_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.success("done").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[OK] done\n");
    }

    #[test]
    fn test_warning_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.warning("careful").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[WARN] careful\n");
    }

    #[test]
    fn test_error_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.error("failed").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[ERROR] failed\n");
    }

    #[test]
    fn test_info_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.info("note").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[INFO] note\n");
    }

    #[test]
    fn test_field_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.field("Status", "Open").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "Status: Open\n");
    }

    #[test]
    fn test_bullet_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.bullet("first item").unwrap();
            fmt.bullet("second item").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "  - first item\n  - second item\n");
    }

    #[test]
    fn test_separator_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.separator().unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output.trim_end(), "-".repeat(80));
    }

    #[test]
    fn test_indent_block_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.indent_block("line 1\nline 2").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "  | line 1\n  | line 2\n");
    }

    #[test]
    fn test_key_hint_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.key_hint("q", "Quit").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[q] Quit\n");
    }

    #[test]
    fn test_progress_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.progress(5, 10).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[##########..........] 50%\n");
    }

    #[test]
    fn test_progress_zero_total() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.progress(0, 0).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "\n");
    }

    #[test]
    fn test_progress_complete() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.progress(10, 10).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("100%"));
        assert!(output.contains("####################"));
    }

    #[test]
    fn test_to_crossterm_color_rgb() {
        let color = to_crossterm_color(RatatuiColor::Rgb(128, 64, 32));
        assert_eq!(
            color,
            CrosstermColor::Rgb {
                r: 128,
                g: 64,
                b: 32
            }
        );
    }

    #[test]
    fn test_to_crossterm_color_indexed() {
        let color = to_crossterm_color(RatatuiColor::Indexed(42));
        assert_eq!(color, CrosstermColor::AnsiValue(42));
    }

    #[test]
    fn test_to_crossterm_color_named() {
        assert_eq!(
            to_crossterm_color(RatatuiColor::Black),
            CrosstermColor::Black
        );
        assert_eq!(
            to_crossterm_color(RatatuiColor::White),
            CrosstermColor::White
        );
        assert_eq!(
            to_crossterm_color(RatatuiColor::Red),
            CrosstermColor::DarkRed
        );
        assert_eq!(
            to_crossterm_color(RatatuiColor::LightRed),
            CrosstermColor::Red
        );
    }

    #[test]
    fn test_terminal_width_returns_reasonable_value() {
        let width = terminal_width();
        assert!(
            width >= 20,
            "Terminal width should be at least 20: got {width}"
        );
        assert!(
            width <= 1000,
            "Terminal width should be at most 1000: got {width}"
        );
    }

    #[test]
    fn test_formatter_accessors() {
        let mut buf = Vec::new();
        let fmt = Formatter::plain(&mut buf);

        assert_eq!(fmt.mode(), OutputMode::Plain);
        assert!(!fmt.is_styled());
        assert_eq!(fmt.width(), 80);
    }

    #[test]
    fn test_formatter_styled_accessors() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        let fmt = Formatter::styled(&mut buf, theme);

        assert_eq!(fmt.mode(), OutputMode::Styled);
        assert!(fmt.is_styled());
    }

    #[test]
    fn test_write_raw_always_plain() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            fmt.write_raw("no escape").unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "no escape");
    }

    #[test]
    fn test_combined_output() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            fmt.heading("Report").unwrap();
            fmt.field("Name", "Test").unwrap();
            fmt.success("All checks passed").unwrap();
            fmt.bullet("Item one").unwrap();
            fmt.bullet("Item two").unwrap();
            fmt.separator().unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("--- Report ---"));
        assert!(output.contains("Name: Test"));
        assert!(output.contains("[OK] All checks passed"));
        assert!(output.contains("  - Item one"));
        assert!(output.contains("  - Item two"));
    }

    #[test]
    fn test_styled_bold_colored() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            fmt.write_bold_colored("bold text", RatatuiColor::Rgb(255, 0, 0))
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("bold text"));
        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_emit_fn() {
        let mut buf = Vec::new();
        emit(
            &mut buf,
            &SetForegroundColor(CrosstermColor::Rgb { r: 255, g: 0, b: 0 }),
        )
        .unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\x1b["));
    }
}
