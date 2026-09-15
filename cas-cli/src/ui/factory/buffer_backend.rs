//! Buffer-based ratatui backend for PTY forwarding
//!
//! This backend captures terminal output to a buffer instead of writing
//! directly to a terminal. The buffer contents can then be sent to clients.

use crossterm::{
    Command,
    cursor::{Hide, MoveTo, SetCursorStyle, Show},
    style::{
        Attribute, Color as CrosstermColor, Print, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
    terminal::{Clear, ClearType},
};
use ghostty_vt::{CursorShape, CursorState};
use ratatui::backend::ClearType as RatatuiClearType;
use ratatui::backend::{Backend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};

pub type HyperlinkMap = Arc<Mutex<BTreeMap<(u16, u16), Arc<str>>>>;

pub fn new_hyperlink_map() -> HyperlinkMap {
    Arc::new(Mutex::new(BTreeMap::new()))
}

/// A ratatui backend that writes to a buffer
pub struct BufferBackend {
    /// Output buffer
    buffer: Vec<u8>,
    /// Reusable scratch buffer for crossterm command serialization
    scratch: Vec<u8>,
    /// Terminal size
    width: u16,
    height: u16,
    /// Per-frame hyperlink metadata keyed by final host terminal coordinates.
    hyperlinks: HyperlinkMap,
}

impl BufferBackend {
    /// Create a new buffer backend with the given size
    #[cfg(test)]
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            buffer: Vec::with_capacity(16384),
            scratch: Vec::with_capacity(64),
            width,
            height,
            hyperlinks: new_hyperlink_map(),
        }
    }

    pub fn with_hyperlinks(width: u16, height: u16, hyperlinks: HyperlinkMap) -> Self {
        Self {
            buffer: Vec::with_capacity(16384),
            scratch: Vec::with_capacity(64),
            width,
            height,
            hyperlinks,
        }
    }

    /// Take the buffered output, leaving an empty buffer
    pub fn take_buffer(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }

    /// Resize the backend
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    /// Forward the child PTY's cursor shape and blink mode to the host.
    pub(crate) fn set_cursor_style(&mut self, state: CursorState) -> io::Result<()> {
        let style = match (state.shape, state.blinking) {
            (CursorShape::Block, true) => SetCursorStyle::BlinkingBlock,
            (CursorShape::Block, false) => SetCursorStyle::SteadyBlock,
            (CursorShape::Underline, true) => SetCursorStyle::BlinkingUnderScore,
            (CursorShape::Underline, false) => SetCursorStyle::SteadyUnderScore,
            (CursorShape::Bar, true) => SetCursorStyle::BlinkingBar,
            (CursorShape::Bar, false) => SetCursorStyle::SteadyBar,
        };
        self.write_command(style)
    }

    /// Write a crossterm command to the buffer (reuses scratch Vec to avoid per-call allocation)
    fn write_command(&mut self, cmd: impl Command) -> io::Result<()> {
        self.scratch.clear();
        crossterm::execute!(self.scratch, cmd)?;
        self.buffer.extend_from_slice(&self.scratch);
        Ok(())
    }

    fn write_osc8_open(&mut self, uri: &str) {
        self.buffer.extend_from_slice(b"\x1b]8;;");
        self.buffer
            .extend(uri.bytes().filter(|b| !matches!(b, b'\x1b' | b'\x07')));
        self.buffer.extend_from_slice(b"\x1b\\");
    }

    fn write_osc8_close(&mut self) {
        self.buffer.extend_from_slice(b"\x1b]8;;\x1b\\");
    }

    fn close_active_link(&mut self, active_link: &mut Option<Arc<str>>) {
        if active_link.take().is_some() {
            self.write_osc8_close();
        }
    }

    /// Convert ratatui color to crossterm color
    fn to_crossterm_color(color: Color) -> CrosstermColor {
        match color {
            Color::Reset => CrosstermColor::Reset,
            Color::Black => CrosstermColor::Black,
            Color::Red => CrosstermColor::DarkRed,
            Color::Green => CrosstermColor::DarkGreen,
            Color::Yellow => CrosstermColor::DarkYellow,
            Color::Blue => CrosstermColor::DarkBlue,
            Color::Magenta => CrosstermColor::DarkMagenta,
            Color::Cyan => CrosstermColor::DarkCyan,
            Color::Gray => CrosstermColor::Grey,
            Color::DarkGray => CrosstermColor::DarkGrey,
            Color::LightRed => CrosstermColor::Red,
            Color::LightGreen => CrosstermColor::Green,
            Color::LightYellow => CrosstermColor::Yellow,
            Color::LightBlue => CrosstermColor::Blue,
            Color::LightMagenta => CrosstermColor::Magenta,
            Color::LightCyan => CrosstermColor::Cyan,
            Color::White => CrosstermColor::White,
            Color::Rgb(r, g, b) => CrosstermColor::Rgb { r, g, b },
            Color::Indexed(i) => CrosstermColor::AnsiValue(i),
        }
    }
}

impl Backend for BufferBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut last_pos: Option<(u16, u16)> = None;
        let mut last_fg = Color::Reset;
        let mut last_bg = Color::Reset;
        let mut last_modifier = Modifier::empty();
        let hyperlinks = self
            .hyperlinks
            .lock()
            .map(|map| map.clone())
            .unwrap_or_default();
        let mut active_link: Option<Arc<str>> = None;

        for (x, y, cell) in content {
            // Move cursor if not contiguous
            if last_pos != Some((x.saturating_sub(1), y)) {
                self.close_active_link(&mut active_link);
                self.write_command(MoveTo(x, y))?;
            }
            last_pos = Some((x, y));

            // Update foreground color if changed
            if cell.fg != last_fg {
                self.write_command(SetForegroundColor(Self::to_crossterm_color(cell.fg)))?;
                last_fg = cell.fg;
            }

            // Update background color if changed
            if cell.bg != last_bg {
                self.write_command(SetBackgroundColor(Self::to_crossterm_color(cell.bg)))?;
                last_bg = cell.bg;
            }

            // Update modifiers if changed
            if cell.modifier != last_modifier {
                // Reset all attributes first
                self.write_command(SetAttribute(Attribute::Reset))?;

                // Reapply colors after reset
                if last_fg != Color::Reset {
                    self.write_command(SetForegroundColor(Self::to_crossterm_color(last_fg)))?;
                }
                if last_bg != Color::Reset {
                    self.write_command(SetBackgroundColor(Self::to_crossterm_color(last_bg)))?;
                }

                // Apply new modifiers
                if cell.modifier.contains(Modifier::BOLD) {
                    self.write_command(SetAttribute(Attribute::Bold))?;
                }
                if cell.modifier.contains(Modifier::DIM) {
                    self.write_command(SetAttribute(Attribute::Dim))?;
                }
                if cell.modifier.contains(Modifier::ITALIC) {
                    self.write_command(SetAttribute(Attribute::Italic))?;
                }
                if cell.modifier.contains(Modifier::UNDERLINED) {
                    self.write_command(SetAttribute(Attribute::Underlined))?;
                }
                if cell.modifier.contains(Modifier::REVERSED) {
                    self.write_command(SetAttribute(Attribute::Reverse))?;
                }
                if cell.modifier.contains(Modifier::CROSSED_OUT) {
                    self.write_command(SetAttribute(Attribute::CrossedOut))?;
                }
                last_modifier = cell.modifier;
            }

            let link = hyperlinks.get(&(x, y));
            if active_link.as_ref() != link {
                self.close_active_link(&mut active_link);
                if let Some(uri) = link {
                    self.write_osc8_open(uri);
                    active_link = Some(Arc::clone(uri));
                }
            }

            // Write the cell content
            self.write_command(Print(cell.symbol()))?;
        }

        self.close_active_link(&mut active_link);

        // Reset attributes at the end
        self.write_command(SetAttribute(Attribute::Reset))?;

        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.write_command(Hide)
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.write_command(Show)
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(Position::new(0, 0))
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        let pos = position.into();
        self.write_command(MoveTo(pos.x, pos.y))
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.write_command(Clear(ClearType::All))
    }

    fn clear_region(&mut self, clear_type: RatatuiClearType) -> Result<(), Self::Error> {
        let crossterm_clear = match clear_type {
            RatatuiClearType::All => ClearType::All,
            RatatuiClearType::AfterCursor => ClearType::FromCursorDown,
            RatatuiClearType::BeforeCursor => ClearType::FromCursorUp,
            RatatuiClearType::CurrentLine => ClearType::CurrentLine,
            RatatuiClearType::UntilNewLine => ClearType::UntilNewLine,
        };
        self.write_command(Clear(crossterm_clear))
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(Size::new(self.width, self.height))
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        // Return window size with cell/pixel dimensions (not really used, but required by trait)
        Ok(WindowSize {
            columns_rows: Size::new(self.width, self.height),
            pixels: Size::new(self.width * 8, self.height * 16),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{BufferBackend, new_hyperlink_map};
    use ghostty_vt::{CursorShape, CursorState};
    use ratatui::backend::Backend;
    use ratatui::buffer::Cell;
    use std::sync::Arc;

    fn cell(symbol: &'static str) -> Cell {
        Cell::new(symbol)
    }

    #[test]
    fn linked_cells_emit_osc8_around_contiguous_runs() {
        let links = new_hyperlink_map();
        {
            let mut map = links.lock().unwrap();
            map.insert((0, 0), Arc::from("https://example.com"));
            map.insert((1, 0), Arc::from("https://example.com"));
        }
        let mut backend = BufferBackend::with_hyperlinks(10, 2, links);
        let cells = [cell("c"), cell("l"), cell("i")];

        backend
            .draw(vec![(0, 0, &cells[0]), (1, 0, &cells[1]), (2, 0, &cells[2])].into_iter())
            .unwrap();

        let output = String::from_utf8(backend.take_buffer()).unwrap();
        assert!(output.contains("\x1b]8;;https://example.com\x1b\\cl\x1b]8;;\x1b\\i"));
    }

    #[test]
    fn different_links_close_and_reopen_at_run_boundaries() {
        let links = new_hyperlink_map();
        {
            let mut map = links.lock().unwrap();
            map.insert((0, 0), Arc::from("https://one.example"));
            map.insert((1, 0), Arc::from("https://two.example"));
        }
        let mut backend = BufferBackend::with_hyperlinks(10, 2, links);
        let cells = [cell("a"), cell("b")];

        backend
            .draw(vec![(0, 0, &cells[0]), (1, 0, &cells[1])].into_iter())
            .unwrap();

        let output = String::from_utf8(backend.take_buffer()).unwrap();
        assert!(output.contains(
            "\x1b]8;;https://one.example\x1b\\a\x1b]8;;\x1b\\\x1b]8;;https://two.example\x1b\\b\x1b]8;;\x1b\\"
        ));
    }

    #[test]
    fn unlinked_cells_emit_no_osc8() {
        let mut backend = BufferBackend::new(10, 2);
        let cells = [cell("x"), cell("y")];

        backend
            .draw(vec![(0, 0, &cells[0]), (1, 0, &cells[1])].into_iter())
            .unwrap();

        let output = String::from_utf8(backend.take_buffer()).unwrap();
        assert!(!output.contains("\x1b]8;;"));
    }

    #[test]
    fn cursor_jumps_close_active_link() {
        let links = new_hyperlink_map();
        {
            let mut map = links.lock().unwrap();
            map.insert((0, 0), Arc::from("https://example.com"));
            map.insert((4, 0), Arc::from("https://example.com"));
        }
        let mut backend = BufferBackend::with_hyperlinks(10, 2, links);
        let cells = [cell("a"), cell("b")];

        backend
            .draw(vec![(0, 0, &cells[0]), (4, 0, &cells[1])].into_iter())
            .unwrap();

        let output = String::from_utf8(backend.take_buffer()).unwrap();
        assert_eq!(
            output.matches("\x1b]8;;https://example.com\x1b\\").count(),
            2
        );
        assert_eq!(output.matches("\x1b]8;;\x1b\\").count(), 2);
    }

    #[test]
    fn osc8_uri_sanitization_strips_esc_and_bel() {
        let links = new_hyperlink_map();
        {
            let mut map = links.lock().unwrap();
            map.insert((0, 0), Arc::from("https://exa\x1bmple.com/\x07bad"));
        }
        let mut backend = BufferBackend::with_hyperlinks(10, 2, links);
        let cells = [cell("x")];

        backend.draw(vec![(0, 0, &cells[0])].into_iter()).unwrap();

        let output = backend.take_buffer();
        assert!(
            output
                .windows(b"https://example.com/bad".len())
                .any(|w| w == b"https://example.com/bad")
        );
        assert!(
            !output
                .windows(b"https://exa\x1bmple.com".len())
                .any(|w| w == b"https://exa\x1bmple.com")
        );
        assert!(
            !output
                .windows(b"com/\x07bad".len())
                .any(|w| w == b"com/\x07bad")
        );
    }

    #[test]
    fn cursor_style_sequences_forward_shape_and_blink() {
        let cases = [
            (CursorShape::Block, false, b"\x1b[2 q".as_slice()),
            (CursorShape::Block, true, b"\x1b[1 q".as_slice()),
            (CursorShape::Underline, false, b"\x1b[4 q".as_slice()),
            (CursorShape::Underline, true, b"\x1b[3 q".as_slice()),
            (CursorShape::Bar, false, b"\x1b[6 q".as_slice()),
            (CursorShape::Bar, true, b"\x1b[5 q".as_slice()),
        ];

        for (shape, blinking, expected) in cases {
            let mut backend = BufferBackend::new(10, 2);
            backend
                .set_cursor_style(CursorState {
                    visible: true,
                    shape,
                    blinking,
                })
                .unwrap();
            assert_eq!(
                backend.take_buffer(),
                expected,
                "shape={shape:?} blinking={blinking}"
            );
        }
    }
}
