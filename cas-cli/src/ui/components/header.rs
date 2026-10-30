//! Header component — styled section headers with levels and optional icons

use std::io;

use super::formatter::Formatter;
use super::traits::Renderable;
use crate::ui::theme::Icons;

/// Heading level controlling visual prominence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Primary heading: bold accent with separator bars (═══ TITLE ═══)
    H1,
    /// Secondary heading: bold primary text
    H2,
    /// Tertiary heading: secondary text, no bold
    H3,
}

/// A styled section header.
///
/// ```ignore
/// let header = Header::new("Tasks", Level::H1);
/// header.render(&mut fmt)?;
/// // ═══ Tasks ═══
///
/// let header = Header::new("Details", Level::H2).with_icon("📋");
/// header.render(&mut fmt)?;
/// // 📋 Details
/// ```
pub struct Header {
    text: String,
    level: Level,
    icon: Option<String>,
}

impl Header {
    /// Create a new header at the given level.
    pub fn new(text: impl Into<String>, level: Level) -> Self {
        Self {
            text: text.into(),
            level,
            icon: None,
        }
    }

    /// Create an H1 header.
    pub fn h1(text: impl Into<String>) -> Self {
        Self::new(text, Level::H1)
    }

    /// Create an H2 header.
    pub fn h2(text: impl Into<String>) -> Self {
        Self::new(text, Level::H2)
    }

    /// Create an H3 header.
    pub fn h3(text: impl Into<String>) -> Self {
        Self::new(text, Level::H3)
    }

    /// Add an icon prefix.
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }
}

impl Renderable for Header {
    fn render(&self, fmt: &mut Formatter) -> io::Result<()> {
        let display_text = match &self.icon {
            Some(icon) => format!("{icon} {}", self.text),
            None => self.text.clone(),
        };

        match self.level {
            Level::H1 => {
                if fmt.is_styled() {
                    let bar = Icons::SEPARATOR_DOUBLE;
                    let color = fmt.theme().palette.accent;
                    fmt.write_bold_colored(
                        &format!("{bar}{bar}{bar} {display_text} {bar}{bar}{bar}"),
                        color,
                    )?;
                } else {
                    fmt.write_raw(&format!("=== {display_text} ==="))?;
                }
            }
            Level::H2 => {
                if fmt.is_styled() {
                    let color = fmt.theme().palette.text_primary;
                    fmt.write_bold_colored(&display_text, color)?;
                } else {
                    fmt.write_raw(&format!("--- {display_text}"))?;
                }
            }
            Level::H3 => {
                let color = fmt.theme().palette.text_secondary;
                fmt.write_colored(&display_text, color)?;
            }
        }

        fmt.newline()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::ActiveTheme;

    #[test]
    fn test_h1_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            Header::h1("Tasks").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "=== Tasks ===\n");

        let mut env = crate::test_support::TestEnvGuard::with_vars(&[("LC_ALL", "C")]);
        for lang_only in [false, true] {
            if lang_only {
                env.remove_var("LC_ALL");
                env.remove_var("LC_CTYPE");
                env.set_var("LANG", "C");
            }
            let mut buf = Vec::new();
            Header::h1("Tasks")
                .render(&mut Formatter::styled(&mut buf, ActiveTheme::default_dark()))
                .unwrap();
            assert_eq!(
                crate::ui::components::test_helpers::strip_ansi_codes(&String::from_utf8(buf).unwrap()),
                "=== Tasks ===\n"
            );
        }
    }

    #[test]
    fn test_h1_with_icon_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            Header::h1("Tasks")
                .with_icon("📋")
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "=== 📋 Tasks ===\n");
    }

    #[test]
    fn test_h2_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            Header::h2("Details").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "--- Details\n");
    }

    #[test]
    fn test_h3_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            Header::h3("Subsection").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "Subsection\n");
    }

    #[test]
    fn test_h1_styled() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("LC_ALL", "C.UTF-8")]);
        for theme in [ActiveTheme::default_dark(), ActiveTheme::default_light()] {
            let mut buf = Vec::new();
            Header::h1("Tasks")
                .render(&mut Formatter::styled(&mut buf, theme))
                .unwrap();
            // Bold only: terminal foreground stays readable on any background.
            assert_eq!(String::from_utf8(buf).unwrap(), "\x1b[1m═══ Tasks ═══\x1b[0m\n");
        }
    }

    #[test]
    fn test_h2_styled() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            Header::h2("Details").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Details"));
        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_h3_styled() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            Header::h3("Sub").render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Sub"));
        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_h2_with_icon_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            Header::h2("Config")
                .with_icon("⚙")
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "--- ⚙ Config\n");
    }

    #[test]
    fn test_new_with_level() {
        let header = Header::new("Test", Level::H1);
        assert_eq!(header.level, Level::H1);
        assert_eq!(header.text, "Test");
        assert!(header.icon.is_none());
    }
}
