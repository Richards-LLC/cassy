//! StatusLine component — icon + status message with semantic coloring

use std::io;

use ratatui::style::Color as RatatuiColor;

use super::formatter::Formatter;
use super::traits::Renderable;
use crate::ui::theme::Icons;

/// Status severity controlling icon and color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Green check mark: ✓
    Success,
    /// Red cross: ✗
    Error,
    /// Yellow warning: ⚠
    Warning,
    /// Blue info: ℹ
    Info,
}

impl Status {
    /// Icon string for this status.
    fn icon(self) -> &'static str {
        match self {
            Status::Success => Icons::CHECK,
            Status::Error => Icons::CROSS,
            Status::Warning => Icons::WARNING,
            Status::Info => Icons::INFO,
        }
    }

    /// Plain-mode prefix tag for this status.
    fn plain_tag(self) -> &'static str {
        match self {
            Status::Success => "[OK]",
            Status::Error => "[ERROR]",
            Status::Warning => "[WARN]",
            Status::Info => "[INFO]",
        }
    }

    /// Resolve the theme color for this status.
    fn color(self, fmt: &Formatter) -> RatatuiColor {
        let palette = &fmt.theme().palette;
        match self {
            Status::Success => palette.status_success,
            Status::Error => palette.status_error,
            Status::Warning => palette.status_warning,
            Status::Info => palette.status_info,
        }
    }
}

/// A single status line with icon, color, and message.
///
/// ```ignore
/// StatusLine::success("All checks passed").render(&mut fmt)?;
/// // ✓ All checks passed
///
/// StatusLine::error("Connection failed").render(&mut fmt)?;
/// // ✗ Connection failed
/// ```
pub struct StatusLine {
    status: Status,
    message: String,
}

impl StatusLine {
    /// Create a status line with the given severity.
    pub fn new(status: Status, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// Create a success status line.
    pub fn success(message: impl Into<String>) -> Self {
        Self::new(Status::Success, message)
    }

    /// Create an error status line.
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Status::Error, message)
    }

    /// Create a warning status line.
    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(Status::Warning, message)
    }

    /// Create an info status line.
    pub fn info(message: impl Into<String>) -> Self {
        Self::new(Status::Info, message)
    }
}

impl Renderable for StatusLine {
    fn render(&self, fmt: &mut Formatter) -> io::Result<()> {
        if fmt.is_styled() {
            let color = self.status.color(fmt);
            let label = if fmt.glyphs() {
                self.status.icon()
            } else {
                self.status.plain_tag()
            };
            fmt.write_colored(&format!("{label} {}", self.message), color)?;
        } else {
            let tag = self.status.plain_tag();
            fmt.write_raw(&format!("{tag} {}", self.message))?;
        }
        fmt.newline()
    }
}

/// Render multiple status lines as a batch.
pub struct StatusGroup {
    lines: Vec<StatusLine>,
}

impl StatusGroup {
    /// Create a new empty status group.
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    /// Append a status line to the group.
    pub fn push(mut self, line: StatusLine) -> Self {
        self.lines.push(line);
        self
    }
}

impl Default for StatusGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderable for StatusGroup {
    fn render(&self, fmt: &mut Formatter) -> io::Result<()> {
        for line in &self.lines {
            line.render(fmt)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::ActiveTheme;

    #[test]
    fn test_success_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            StatusLine::success("done").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[OK] done\n");
    }

    #[test]
    fn test_error_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            StatusLine::error("failed").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[ERROR] failed\n");
    }

    #[test]
    fn test_warning_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            StatusLine::warning("be careful").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[WARN] be careful\n");
    }

    #[test]
    fn warning_styled_uses_ascii_tag_under_c_locale() {
        let mut env = crate::test_support::TestEnvGuard::with_vars(&[("LC_ALL", "C")]);
        env.remove("NO_COLOR");
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, ActiveTheme::default_dark());
            StatusLine::warning("configuration refused").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("[WARN]"), "{output:?}");
        assert!(!output.contains('⚠'), "{output:?}");
    }

    #[test]
    fn test_info_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            StatusLine::info("note this").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[INFO] note this\n");
    }

    #[test]
    fn test_success_styled() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            StatusLine::success("ok").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\x1b["));
        assert!(output.contains("ok"));
    }

    #[test]
    fn test_error_styled() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            StatusLine::error("bad").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\x1b["));
        assert!(output.contains("bad"));
    }

    #[test]
    fn test_status_group_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            StatusGroup::new()
                .push(StatusLine::success("step 1"))
                .push(StatusLine::success("step 2"))
                .push(StatusLine::error("step 3"))
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "[OK] step 1\n[OK] step 2\n[ERROR] step 3\n");
    }

    #[test]
    fn test_status_group_styled() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            StatusGroup::new()
                .push(StatusLine::info("a"))
                .push(StatusLine::warning("b"))
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("a"));
        assert!(output.contains("b"));
        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_status_group_empty() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            StatusGroup::new().render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn test_new_with_status() {
        let line = StatusLine::new(Status::Warning, "watch out");
        assert_eq!(line.status, Status::Warning);
        assert_eq!(line.message, "watch out");
    }
}
