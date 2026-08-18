//! KeyValue component — aligned key-value pairs with semantic coloring

use std::io;

use ratatui::style::Color as RatatuiColor;

use super::formatter::Formatter;
use super::traits::Renderable;

/// A single key-value entry with optional value color override.
pub struct Entry {
    key: String,
    value: String,
    value_color: Option<RatatuiColor>,
}

/// Aligned key-value pair display.
///
/// Keys are right-padded to the longest key width, then followed by `: value`.
/// Keys use secondary text color; values use primary text color (or an override).
///
/// ```ignore
/// KeyValue::new()
///     .add("Status", "Open")
///     .add("Priority", "High")
///     .add("ID", "cas-1234")
///     .render(&mut fmt)?;
///
/// // Output:
/// //   Status: Open
/// // Priority: High
/// //       ID: cas-1234
/// ```
pub struct KeyValue {
    entries: Vec<Entry>,
    separator: String,
}

impl KeyValue {
    /// Create a new empty key-value display.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            separator: ": ".to_string(),
        }
    }

    /// Set a custom separator (default: ": ").
    pub fn with_separator(mut self, sep: impl Into<String>) -> Self {
        self.separator = sep.into();
        self
    }

    /// Add a key-value pair.
    pub fn add(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push(Entry {
            key: key.into(),
            value: value.into(),
            value_color: None,
        });
        self
    }

    /// Add a key-value pair with a custom value color.
    pub fn add_colored(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
        color: RatatuiColor,
    ) -> Self {
        self.entries.push(Entry {
            key: key.into(),
            value: value.into(),
            value_color: Some(color),
        });
        self
    }

    /// Compute the maximum key width for alignment.
    fn max_key_width(&self) -> usize {
        self.entries.iter().map(|e| e.key.len()).max().unwrap_or(0)
    }
}

impl Default for KeyValue {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderable for KeyValue {
    fn render(&self, fmt: &mut Formatter) -> io::Result<()> {
        let max_width = self.max_key_width();

        for entry in &self.entries {
            let padded_key = format!("{:>width$}", entry.key, width = max_width);

            if fmt.is_styled() {
                let key_color = fmt.theme().palette.text_secondary;
                fmt.write_colored(&padded_key, key_color)?;
                fmt.write_raw(&self.separator)?;

                match entry.value_color {
                    Some(color) => fmt.write_colored(&entry.value, color)?,
                    None => fmt.write_primary(&entry.value)?,
                }
            } else {
                fmt.write_raw(&padded_key)?;
                fmt.write_raw(&self.separator)?;
                fmt.write_raw(&entry.value)?;
            }

            fmt.newline()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::ActiveTheme;

    #[test]
    fn test_single_entry_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            KeyValue::new()
                .add("Status", "Open")
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "Status: Open\n");
    }

    #[test]
    fn test_alignment_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            KeyValue::new()
                .add("ID", "cas-1234")
                .add("Status", "Open")
                .add("Priority", "High")
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3);
        // "Priority" is 8 chars, so all keys should be right-padded to 8
        assert_eq!(lines[0], "      ID: cas-1234");
        assert_eq!(lines[1], "  Status: Open");
        assert_eq!(lines[2], "Priority: High");
    }

    #[test]
    fn test_custom_separator_plain() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            KeyValue::new()
                .with_separator(" = ")
                .add("x", "1")
                .add("y", "2")
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "x = 1\ny = 2\n");
    }

    #[test]
    fn test_empty_renders_nothing() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            KeyValue::new().render(&mut fmt).unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn test_styled_output() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme);
            KeyValue::new()
                .add("Name", "Cassy")
                .add("Version", "0.7.0")
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Name"));
        assert!(output.contains("Cassy"));
        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_colored_value_styled() {
        let theme = ActiveTheme::default_dark();
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::styled(&mut buf, theme.clone());
            KeyValue::new()
                .add("Normal", "text")
                .add_colored("Status", "Active", theme.palette.status_success)
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Normal"));
        assert!(output.contains("Active"));
        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_colored_value_plain_ignores_color() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            KeyValue::new()
                .add_colored("Status", "Active", RatatuiColor::Green)
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "Status: Active\n");
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn test_alignment_with_varying_lengths() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            KeyValue::new()
                .add("A", "short")
                .add("LongKeyName", "value")
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "          A: short");
        assert_eq!(lines[1], "LongKeyName: value");
    }

    #[test]
    fn test_single_entry_no_padding() {
        let mut buf = Vec::new();
        {
            let mut fmt = Formatter::plain(&mut buf);
            KeyValue::new()
                .add("Key", "Value")
                .render(&mut fmt)
                .unwrap();
        }
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "Key: Value\n");
    }
}
