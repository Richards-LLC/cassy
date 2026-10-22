//! Interactive prompts for CLI commands

use std::io::{self, BufRead, Write};

use crate::ui::components::OutputMode;
use crossterm::event::KeyModifiers;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode},
};

/// Read a line from stdin with a prompt
pub fn prompt(message: &str) -> io::Result<String> {
    write!(io::stdout(), "{message}: ")?;
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

/// Read a line from stdin with a prompt and default value
pub fn prompt_default(message: &str, default: &str) -> io::Result<String> {
    write!(io::stdout(), "{message} [{default}]: ")?;
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input.to_string())
    }
}

/// Read optional input (empty string allowed)
pub fn prompt_optional(message: &str) -> io::Result<Option<String>> {
    write!(io::stdout(), "{message} (optional): ")?;
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        Ok(None)
    } else {
        Ok(Some(input.to_string()))
    }
}

/// Read multi-line content until empty line
pub fn prompt_multiline(message: &str) -> io::Result<String> {
    writeln!(io::stdout(), "{message}:")?;
    writeln!(
        io::stdout(),
        "(Enter text, then press Enter twice to finish)"
    )?;

    let mut lines = Vec::new();
    let stdin = io::stdin();

    loop {
        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let line = input.trim_end_matches('\n').trim_end_matches('\r');

        if line.is_empty() && !lines.is_empty() {
            break;
        }

        lines.push(line.to_string());
    }

    Ok(lines.join("\n").trim().to_string())
}

/// Present options and get selection
pub fn select(message: &str, options: &[&str]) -> io::Result<usize> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    select_from(message, options, &mut stdin.lock(), &mut stdout.lock())
}

/// Core of `select()` parameterized over reader/writer so the EOF-handling
/// path can be exercised in unit tests without touching real stdin.
pub(crate) fn select_from(
    message: &str,
    options: &[&str],
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> io::Result<usize> {
    writeln!(writer, "{message}:")?;
    for (i, option) in options.iter().enumerate() {
        writeln!(writer, "  {}. {}", i + 1, option)?;
    }

    loop {
        write!(writer, "Select (1-{}): ", options.len())?;
        writer.flush()?;

        let mut input = String::new();
        let bytes = reader.read_line(&mut input)?;

        // `read_line` returns Ok(0) on EOF without setting an error. Without
        // this guard, a closed/EOF'd stdin causes an infinite loop that spins
        // at 100% CPU, re-printing the prompt and re-reading EOF forever.
        // Seen in production: `cas init` hang with 0-byte log file (cas-bf06).
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "select: stdin closed before a selection was made",
            ));
        }

        if let Ok(n) = input.trim().parse::<usize>() {
            if n >= 1 && n <= options.len() {
                return Ok(n - 1);
            }
        }

        writeln!(writer, "Invalid selection, please try again.")?;
    }
}

/// Prompt for a number with default value
pub fn prompt_number(message: &str, default: usize) -> io::Result<usize> {
    let input = prompt_default(message, &default.to_string())?;
    Ok(input.parse().unwrap_or(default))
}

/// Prompt for an i32 number with default value
pub fn prompt_i32(message: &str, default: i32) -> io::Result<i32> {
    let input = prompt_default(message, &default.to_string())?;
    Ok(input.parse().unwrap_or(default))
}

/// Confirm yes/no
pub fn confirm(message: &str, default: bool) -> io::Result<bool> {
    let prompt_text = if default {
        format!("{message} [Y/n]")
    } else {
        format!("{message} [y/N]")
    };

    write!(io::stdout(), "{prompt_text}: ")?;
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    Ok(match input.as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        "" => default,
        _ => default,
    })
}

/// A styled column value within a picker row.
pub struct PickerTag {
    pub text: String,
    pub color: Color,
}

/// An item (row) in the interactive picker.
pub struct PickerItem {
    /// Main label (first column, highlighted when selected)
    pub label: String,
    /// Additional columns rendered after the label, each with its own color
    pub tags: Vec<PickerTag>,
}

/// Pre-computed column widths for aligned rendering.
struct ColumnWidths {
    label: usize,
    tags: Vec<usize>,
}

impl ColumnWidths {
    fn compute(items: &[PickerItem]) -> Self {
        let label = items.iter().map(|i| i.label.len()).max().unwrap_or(0);
        let max_tags = items.iter().map(|i| i.tags.len()).max().unwrap_or(0);
        let mut tags = vec![0usize; max_tags];
        for item in items {
            for (col, tag) in item.tags.iter().enumerate() {
                tags[col] = tags[col].max(tag.text.len());
            }
        }
        Self { label, tags }
    }
}

/// Bubbletea-style inline picker. Returns the selected index, or None if cancelled.
///
/// Renders inline (no alternate screen) with aligned columns, supports
/// arrow keys / j/k, enter to confirm, q/esc/ctrl-c to cancel.
/// Cleans up after itself and prints a completed-form summary line.
pub fn pick(title: &str, items: &[PickerItem]) -> io::Result<Option<usize>> {
    if items.is_empty() {
        return Ok(None);
    }

    let mut stdout = io::stdout();
    let mut selected: usize = 0;
    let cols = ColumnWidths::compute(items);
    let mode = OutputMode::detect();

    // Reserve vertical space so the terminal scrolls enough for our content.
    let total_lines = items.len() + 3; // title + blank + items + help
    for _ in 0..total_lines {
        writeln!(stdout)?;
    }
    stdout.flush()?;

    let (_, cur_row) = crossterm::cursor::position()?;
    let origin_row = cur_row.saturating_sub(total_lines as u16);

    enable_raw_mode()?;
    execute!(stdout, Hide)?;

    render_picker(&mut stdout, title, items, selected, origin_row, &cols, mode)?;

    let result = loop {
        if !event::poll(std::time::Duration::from_millis(50))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                break None;
            }

            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = if selected == 0 {
                        items.len() - 1
                    } else {
                        selected - 1
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = if selected >= items.len() - 1 {
                        0
                    } else {
                        selected + 1
                    };
                }
                KeyCode::Enter => break Some(selected),
                KeyCode::Esc | KeyCode::Char('q') => break None,
                _ => continue,
            }
            render_picker(&mut stdout, title, items, selected, origin_row, &cols, mode)?;
        }
    };

    disable_raw_mode()?;
    execute!(stdout, Show)?;

    // Clear picker area
    for r in 0..total_lines {
        execute!(
            stdout,
            MoveTo(0, origin_row + r as u16),
            Clear(ClearType::CurrentLine),
        )?;
    }
    execute!(stdout, MoveTo(0, origin_row))?;

    render_completed(
        &mut stdout,
        title,
        result.map(|idx| items[idx].label.as_str()),
        mode,
    )?;
    Ok(result)
}

fn render_completed(
    mut stdout: &mut impl Write,
    title: &str,
    selected: Option<&str>,
    _mode: OutputMode,
) -> io::Result<()> {
    // The same summary renderer is used after acceptance and cancellation.
    match selected {
        Some(label) => {
            execute!(
                stdout,
                SetForegroundColor(Color::Green),
                Print("  ✓ "),
                ResetColor,
                SetAttribute(Attribute::Bold),
                Print(title),
                SetAttribute(Attribute::Reset),
                SetForegroundColor(Color::DarkGrey),
                Print(": "),
                ResetColor,
                Print(label),
                Print("\n"),
            )?;
        }
        None => {
            execute!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print("  ✗ "),
                Print(title),
                Print(": cancelled"),
                ResetColor,
                Print("\n"),
            )?;
        }
    }
    Ok(())
}

/// Pad `s` with spaces to `width`.
fn pad(s: &str, width: usize) -> String {
    if s.len() >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - s.len()))
    }
}

fn render_picker(
    mut stdout: &mut impl Write,
    title: &str,
    items: &[PickerItem],
    selected: usize,
    origin_row: u16,
    cols: &ColumnWidths,
    _mode: OutputMode,
) -> io::Result<()> {
    let mut row = origin_row;

    // Title
    queue!(
        stdout,
        MoveTo(0, row),
        Clear(ClearType::CurrentLine),
        SetAttribute(Attribute::Bold),
        Print(format!("  {title}")),
        SetAttribute(Attribute::Reset),
    )?;
    row += 1;

    // Blank line
    queue!(stdout, MoveTo(0, row), Clear(ClearType::CurrentLine))?;
    row += 1;

    // Rows
    for (i, item) in items.iter().enumerate() {
        queue!(stdout, MoveTo(0, row), Clear(ClearType::CurrentLine))?;

        let label_padded = pad(&item.label, cols.label);

        if i == selected {
            // Cursor + bold label column
            queue!(
                stdout,
                SetForegroundColor(Color::Magenta),
                SetAttribute(Attribute::Bold),
                Print("  ❯ "),
                SetAttribute(Attribute::Reset),
                SetAttribute(Attribute::Bold),
                Print(&label_padded),
                SetAttribute(Attribute::Reset),
            )?;
            // Tag columns in their own colors, padded
            for (col, tag) in item.tags.iter().enumerate() {
                let w = cols.tags.get(col).copied().unwrap_or(0);
                queue!(
                    stdout,
                    Print("  "),
                    SetForegroundColor(tag.color),
                    Print(pad(&tag.text, w)),
                    ResetColor,
                )?;
            }
        } else {
            // Dimmed label column
            queue!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print(format!("    {label_padded}")),
            )?;
            // Dimmed tag columns, padded
            for (col, tag) in item.tags.iter().enumerate() {
                let w = cols.tags.get(col).copied().unwrap_or(0);
                queue!(stdout, Print(format!("  {}", pad(&tag.text, w))))?;
            }
            queue!(stdout, ResetColor)?;
        }

        row += 1;
    }

    // Help line
    queue!(
        stdout,
        MoveTo(0, row),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::DarkGrey),
        Print("  ↑/↓ navigate • enter select • q cancel"),
        ResetColor,
    )?;

    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::select_from;
    use std::io::{self, Cursor};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// pin: styled UTF-8 picker output stays identical; C/plain decorations
    /// must be ASCII on the same actual row/help/completion renderers.
    #[test]
    fn picker_decorations_follow_locale_cas_ba0e() {
        use super::{ColumnWidths, PickerItem, PickerTag, render_completed, render_picker};
        use crate::ui::components::OutputMode;
        use crossterm::style::Color;
        let mut env = crate::test_support::TestEnvGuard::new();
        let items = vec![
            PickerItem {
                label: "Alpha".into(),
                tags: vec![PickerTag {
                    text: "ready".into(),
                    color: Color::Green,
                }],
            },
            PickerItem {
                label: "Beta".into(),
                tags: vec![],
            },
        ];
        let cols = ColumnWidths::compute(&items);
        let mut failures = Vec::new();
        for locale in ["C.UTF-8", "C"] {
            env.set("LC_ALL", locale);
            let mut output = Vec::new();
            render_picker(
                &mut output,
                "Destination",
                &items,
                0,
                0,
                &cols,
                OutputMode::Styled,
            )
            .unwrap();
            render_completed(
                &mut output,
                "Destination",
                Some("Alpha"),
                OutputMode::Styled,
            )
            .unwrap();
            render_completed(&mut output, "Destination", None, OutputMode::Styled).unwrap();
            let text = String::from_utf8(output).unwrap();
            if locale == "C.UTF-8" {
                // Save the pre-fix bytes in the red receipt before pinning them.
                eprintln!(
                    "CAS_BA0E_UTF8_RENDER={}",
                    serde_json::to_string(&text).unwrap()
                );
                assert!(text.contains("  ❯ ") && text.contains("  ✓ ") && text.contains("  ✗ "));
                assert!(text.contains("  ↑/↓ navigate • enter select • q cancel"));
            } else if !text.is_ascii() {
                failures.push(format!("{locale}: {text:?}"));
            }
        }
        assert!(
            failures.is_empty(),
            "picker locale fallback missing: {}",
            failures.join("; ")
        );
    }

    /// EOF on stdin must NOT cause `select` to loop forever (cas-bf06).
    /// Prior to this fix, a closed stdin spun at 100% CPU writing
    /// "Invalid selection, please try again." in a tight loop.
    #[test]
    fn select_returns_error_on_eof() {
        let mut reader: Cursor<&[u8]> = Cursor::new(b"");
        let mut writer = Vec::new();

        let result = select_from("pick one", &["A", "B"], &mut reader, &mut writer);

        let err = result.expect_err("select must error on EOF, not loop forever");
        assert_eq!(
            err.kind(),
            io::ErrorKind::UnexpectedEof,
            "EOF on stdin must surface as UnexpectedEof, got {err:?}"
        );
    }

    /// A valid selection returns the 0-based index.
    #[test]
    fn select_returns_index_on_valid_input() {
        let mut reader = Cursor::new(b"2\n".as_slice());
        let mut writer = Vec::new();

        let result = select_from("pick one", &["A", "B"], &mut reader, &mut writer).unwrap();

        assert_eq!(result, 1);
    }

    /// Invalid input retries, but a follow-up EOF terminates instead of spinning.
    /// This is the regression test for cas-bf06: even if the user types
    /// garbage first, an EOF after that must still cause a bounded exit.
    #[test]
    fn select_bounded_on_eof_after_invalid_input() {
        // First a non-numeric line, then EOF.
        let mut reader = Cursor::new(b"banana\n".as_slice());
        let mut writer = Vec::new();

        let started = Instant::now();
        let result = select_from("pick one", &["A", "B"], &mut reader, &mut writer);
        let elapsed = started.elapsed();

        assert!(result.is_err(), "EOF after bad input must error out");
        assert!(
            elapsed < Duration::from_secs(1),
            "select must exit promptly on EOF, took {elapsed:?}"
        );
    }

    /// `read_line` that keeps returning Ok(0) forever (the real OS behavior
    /// on EOF) must not cause an infinite loop. This reader explicitly counts
    /// its calls so a regression would make the test hang *and* fail the
    /// call-count assertion if it ever completed.
    #[test]
    fn select_does_not_spin_on_sticky_eof() {
        struct StickyEof {
            calls: Arc<AtomicUsize>,
        }
        impl io::Read for StickyEof {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let reader = StickyEof {
            calls: Arc::clone(&calls),
        };
        let mut reader = io::BufReader::new(reader);
        let mut writer = Vec::new();

        // Run in a thread so a regression (infinite loop) can be detected
        // without hanging the whole test binary.
        let done = Arc::new(Mutex::new(false));
        let done_clone = Arc::clone(&done);
        let handle = std::thread::spawn(move || {
            let r = select_from("pick one", &["A", "B"], &mut reader, &mut writer);
            *done_clone.lock().unwrap() = true;
            r
        });

        // Give the function 2 seconds to return. Fix returns immediately on
        // first EOF; regression would loop forever and we'd timeout here.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && !*done.lock().unwrap() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            *done.lock().unwrap(),
            "select hung on sticky EOF — regression of cas-bf06"
        );

        let result = handle.join().unwrap();
        assert!(result.is_err());
        assert!(
            calls.load(Ordering::SeqCst) <= 2,
            "select must not retry EOF reads: {} read calls",
            calls.load(Ordering::SeqCst)
        );
    }
}
