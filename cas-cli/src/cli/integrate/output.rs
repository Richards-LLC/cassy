//! Width and locale policy at the human-output boundary, shared by every
//! integration platform and Violet's rejected-credential diagnostics.

use std::io::{self, IsTerminal, Write};

use ratatui::text::Span;

use super::types::IntegrationOutcome;
use crate::ui::components::{ascii_fallback, locale_is_utf8};

fn resolved_width(tty: Option<u16>, columns: Option<&str>) -> usize {
    tty.filter(|width| *width >= 40)
        .or_else(|| {
            columns
                .and_then(|value| value.trim().parse::<u16>().ok())
                .filter(|width| *width >= 40)
        })
        .unwrap_or(80) as usize
}

fn width() -> usize {
    let tty = std::io::stdout()
        .is_terminal()
        .then(|| crossterm::terminal::size().ok().map(|(width, _)| width))
        .flatten();
    resolved_width(tty, std::env::var("COLUMNS").ok().as_deref())
}

struct Rows<'a, W> {
    writer: &'a mut W,
    width: usize,
    utf8: bool,
    full: bool,
    shortened: bool,
}

impl<W: Write> Rows<'_, W> {
    fn row(&mut self, text: &str, indent: usize) -> io::Result<()> {
        let safe = if self.utf8 {
            text.to_string()
        } else {
            ascii_fallback(text)
                .chars()
                .flat_map(|ch| {
                    if ch.is_ascii() {
                        ch.to_string().chars().collect::<Vec<_>>()
                    } else {
                        ch.escape_unicode().collect::<Vec<_>>()
                    }
                })
                .collect()
        };
        for paragraph in safe.lines() {
            let continuation = indent + 2;
            let budget = self.width.saturating_sub(continuation);
            let mut line = " ".repeat(indent);
            for token in paragraph.split_whitespace() {
                let token = if !self.full && cells(token) > budget {
                    self.shortened = true;
                    shorten(token, budget, if self.utf8 { "…" } else { "..." })
                } else {
                    token.to_string()
                };
                let separator = usize::from(!line.trim().is_empty());
                if cells(&line) + separator + cells(&token) > self.width && !line.trim().is_empty()
                {
                    writeln!(self.writer, "{line}")?;
                    line = " ".repeat(continuation);
                }
                if !line.trim().is_empty() {
                    line.push(' ');
                }
                line.push_str(&token);
            }
            writeln!(self.writer, "{line}")?;
        }
        Ok(())
    }

    fn summaries(&mut self, summary: &[String]) -> io::Result<()> {
        for line in summary
            .iter()
            .filter(|line| line.starts_with("next:"))
            .chain(summary.iter().filter(|line| !line.starts_with("next:")))
        {
            self.row(line, 2)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.shortened {
            self.row("--full for untruncated values", 2)?;
        }
        Ok(())
    }
}

fn cells(text: &str) -> usize {
    Span::raw(text).width()
}

fn shorten(token: &str, width: usize, mark: &str) -> String {
    let spare = width.saturating_sub(cells(mark));
    let left = spare / 2;
    let right = spare - left;
    let mut prefix = String::new();
    for ch in token.chars() {
        let next = format!("{prefix}{ch}");
        if cells(&next) > left {
            break;
        }
        prefix.push(ch);
    }
    let mut suffix = String::new();
    for ch in token.chars().rev() {
        let next = format!("{ch}{suffix}");
        if cells(&next) > right {
            break;
        }
        suffix = next;
    }
    format!("{prefix}{mark}{suffix}")
}

pub(super) fn render(
    outcome: &IntegrationOutcome,
    writer: &mut impl Write,
    full: bool,
) -> io::Result<()> {
    let mut rows = Rows {
        writer,
        width: width(),
        utf8: locale_is_utf8(),
        full,
        shortened: false,
    };
    rows.row(
        &format!(
            "{} {}: {}",
            outcome.platform.as_str(),
            outcome.action.as_str(),
            outcome.status.as_str()
        ),
        0,
    )?;
    rows.summaries(&outcome.summary)?;
    for file in &outcome.files {
        rows.row(&format!("wrote {}", file.display()), 2)?;
    }
    rows.finish()
}

pub(super) fn render_summary(
    summary: &[String],
    writer: &mut impl Write,
    full: bool,
) -> io::Result<()> {
    let mut rows = Rows {
        writer,
        width: width(),
        utf8: locale_is_utf8(),
        full,
        shortened: false,
    };
    rows.summaries(summary)?;
    rows.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_terminal_width_and_fallback_cas_46b0() {
        assert_eq!(resolved_width(Some(40), Some("120")), 40);
        assert_eq!(resolved_width(Some(120), Some("40")), 120);
        assert_eq!(resolved_width(None, Some("40")), 40);
        for invalid in ["0", "20", "bad", "999999"] {
            assert_eq!(resolved_width(None, Some(invalid)), 80);
        }
        assert_eq!(resolved_width(None, None), 80);
    }

    #[test]
    fn unicode_display_width_and_ascii_content_cas_46b0() {
        for utf8 in [true, false] {
            let mut output = Vec::new();
            let mut rows = Rows {
                writer: &mut output,
                width: 40,
                utf8,
                full: false,
                shortened: false,
            };
            rows.row(
                &format!("project: {} — preserve identity", "界e\u{301}".repeat(60)),
                2,
            )
            .unwrap();
            rows.finish().unwrap();
            let text = String::from_utf8(output).unwrap();
            assert!(text.lines().all(|line| cells(line) <= 40), "{text}");
            assert!(text.contains("--full for untruncated values"));
            if !utf8 {
                assert!(text.is_ascii());
            }
        }
    }
}
