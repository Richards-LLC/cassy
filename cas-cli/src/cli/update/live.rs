//! Phase lines for `cas update` (cas-0aa5).
//!
//! A phase owns its line. On a terminal, the line redraws in place while the
//! phase runs (spinner, label, what it is doing, elapsed), and the phase's own
//! output is captured so nothing interleaves with it. When the phase ends, the
//! line is replaced by its result in the same columns, then anything the
//! phase printed that the result does not already say. Piped, the phase
//! appends one start line and one result line, and nothing redraws.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::ui::components::formatter::{locale_is_utf8, terminal_width};
use crate::ui::components::{Formatter, OutputMode, Verdict};
use crate::ui::theme::{ActiveTheme, Icons};

/// What the running phase is doing now ("7 of 18 · gabber-studio"), drawn
/// on its live line. One phase runs at a time, so one slot is enough.
static ACTIVITY: Mutex<Option<String>> = Mutex::new(None);

/// Say what the running phase is doing; shown on its live line.
pub(super) fn set_activity(text: impl Into<String>) {
    if let Ok(mut slot) = ACTIVITY.lock() {
        *slot = Some(text.into());
    }
}

fn take_activity() -> Option<String> {
    ACTIVITY.lock().ok().and_then(|mut slot| slot.take())
}

fn activity() -> Option<String> {
    ACTIVITY.lock().ok().and_then(|slot| slot.clone())
}

/// The phase that holds the terminal without capturing it (it may prompt),
/// so work inside it can still draw a live line once its prompt is answered.
struct OpenPhase {
    mode: OutputMode,
    counter: String,
    label: String,
}

static OPEN_PHASE: Mutex<Option<OpenPhase>> = Mutex::new(None);

/// Record (or, with `None`, clear) the uncaptured phase now running.
pub(super) fn set_open_phase(phase: Option<(OutputMode, String, String)>) {
    if let Ok(mut slot) = OPEN_PHASE.lock() {
        *slot = phase.map(|(mode, counter, label)| OpenPhase { mode, counter, label });
    }
}

/// Redraw the open phase's line with `doing` until the guard drops
/// (cas-70a8: the download after an interactive confirmation). Nothing is
/// drawn when stdout cannot animate or no uncaptured phase is open.
pub(super) fn during_open_phase(doing: impl Into<String>) -> Option<LiveLine> {
    if !can_animate() {
        return None;
    }
    let (mode, counter, label) = {
        let slot = OPEN_PHASE.lock().ok()?;
        let phase = slot.as_ref()?;
        (phase.mode, phase.counter.clone(), phase.label.clone())
    };
    let terminal = super::terminal_handle()?;
    Some(LiveLine::start(terminal, mode, counter, label, doing.into()))
}

/// Phase lines align on these columns: mark, counter, label, text, elapsed.
const LABEL_CELLS: usize = 14;
const LINE_CELLS: usize = 80;

/// The result of a finished phase, drawn where its live line was.
pub(super) struct PhaseResult {
    pub(super) verdict: Verdict,
    pub(super) text: String,
}

/// Whether a phase line may animate: stdout is a terminal that can redraw.
pub(super) fn can_animate() -> bool {
    io::stdout().is_terminal() && std::env::var("TERM").map_or(true, |term| term != "dumb")
}

/// Render one phase line (no newline) into `fmt`. `mark` is `None` while the
/// phase runs piped: the start line carries an arrow instead of a verdict.
pub(super) fn render_line(
    fmt: &mut Formatter<'_>,
    mark: Option<Verdict>,
    spinner: Option<&str>,
    counter: &str,
    label: &str,
    text: &str,
    elapsed: Option<Duration>,
) -> io::Result<()> {
    let unicode = fmt.unicode();
    let width = usize::from(fmt.width()).clamp(40, LINE_CELLS);
    let mark_cells = match (mark, spinner) {
        (Some(verdict), _) => {
            let cells = verdict.label(fmt.glyphs()).chars().count();
            fmt.mark(verdict)?;
            cells
        }
        (None, Some(frame)) => {
            if fmt.is_styled() {
                let accent = fmt.theme().palette.text_accent;
                fmt.write_colored(frame, accent)?;
            } else {
                fmt.write_raw(frame)?;
            }
            1
        }
        (None, None) => {
            let arrow = Icons::arrow_right(unicode);
            fmt.write_muted(arrow)?;
            arrow.chars().count()
        }
    };
    // Every phase line puts its counter in the same column, whatever mark
    // the mode draws (`✓`, `[OK]`, `[ERROR]`, a spinner frame, an arrow).
    let mark_column = super::findings::mark_cells(fmt.glyphs());
    fmt.write_raw(&" ".repeat(mark_column.saturating_sub(mark_cells) + 1))?;
    fmt.write_muted(counter)?;
    fmt.write_raw(" ")?;
    // Padding is also the label/status separator, even when the label fills
    // or exceeds the usual column. Include that cell in the width budget.
    let label_cells = (label.chars().count() + 1).max(LABEL_CELLS);
    let label = format!("{label:<label_cells$}");
    fmt.write_bold_plain(&label)?;
    let used = mark_column.max(mark_cells) + 1 + counter.chars().count() + 1 + label_cells;
    let elapsed = elapsed.map(super::format_elapsed).unwrap_or_default();
    let room = width.saturating_sub(used + elapsed.chars().count() + 2);
    let text = fit(text, room, unicode);
    fmt.write_text(&text)?;
    if !elapsed.is_empty() {
        let pad = width.saturating_sub(used + text.chars().count() + elapsed.chars().count());
        fmt.write_raw(&" ".repeat(pad.max(2)))?;
        fmt.write_muted(&elapsed)?;
    }
    Ok(())
}

/// Truncate `text` to `cells` with an ellipsis. Phase texts are short by
/// construction; this only guards an unexpectedly long version or path.
fn fit(text: &str, cells: usize, unicode: bool) -> String {
    if text.chars().count() <= cells {
        return text.to_owned();
    }
    let ellipsis = if unicode { "…" } else { "..." };
    let keep = cells.saturating_sub(ellipsis.chars().count());
    format!("{}{ellipsis}", text.chars().take(keep).collect::<String>())
}

/// A phase line being redrawn by a background thread while its phase runs.
pub(super) struct LiveLine {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LiveLine {
    /// Start redrawing on the terminal. `terminal` must be the real stdout,
    /// taken before the phase's output is captured.
    pub(super) fn start(
        mut terminal: std::fs::File,
        mode: OutputMode,
        counter: String,
        label: String,
        doing: String,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let width = terminal_width();
        let frames: &[&str] = if locale_is_utf8() {
            &Icons::SPINNER
        } else {
            &["|", "/", "-", "\\"]
        };
        let handle = std::thread::spawn(move || {
            let started = Instant::now();
            let mut frame = 0usize;
            while !flag.load(Ordering::Relaxed) {
                let mut bytes = Vec::new();
                bytes.extend_from_slice(b"\r\x1b[2K");
                {
                    let mut fmt = Formatter::new(&mut bytes, mode, ActiveTheme::default(), width);
                    let now = activity().unwrap_or_else(|| doing.clone());
                    let _ = render_line(
                        &mut fmt,
                        None,
                        Some(frames[frame % frames.len()]),
                        &counter,
                        &label,
                        &now,
                        Some(started.elapsed()),
                    );
                }
                let _ = terminal.write_all(&bytes);
                let _ = terminal.flush();
                frame += 1;
                std::thread::sleep(Duration::from_millis(80));
            }
            let _ = terminal.write_all(b"\r\x1b[2K");
            let _ = terminal.flush();
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Stop redrawing and clear the line, leaving the cursor at its start.
    pub(super) fn finish(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        take_activity();
    }
}

impl Drop for LiveLine {
    fn drop(&mut self) {
        self.halt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_status_keeps_a_gap_after_full_width_labels_cas_f1354() {
        for label in ["Updating files", "Distributing built-ins to user-level"] {
            for (mark, status, elapsed) in [
                (None, "working", None),
                (Some(Verdict::Ok), "done", Some(Duration::from_millis(194))),
            ] {
                let mut bytes = Vec::new();
                let mut fmt =
                    Formatter::new(&mut bytes, OutputMode::Plain, ActiveTheme::default(), 80);
                render_line(&mut fmt, mark, None, "[1/1]", label, status, elapsed).unwrap();
                let row = String::from_utf8(bytes).unwrap();
                let after_label = row.split_once(label).expect("phase label").1;
                assert!(
                    after_label.starts_with(' '),
                    "label and status joined: {row:?}"
                );
                assert_eq!(after_label.split_whitespace().next(), Some(status), "{row:?}");
                assert!(ratatui::text::Line::from(row.as_str()).width() <= 80, "{row:?}");
            }
        }
    }
}
