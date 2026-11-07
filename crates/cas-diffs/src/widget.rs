//! Ratatui widgets for rendering diffs in the terminal.
//!
//! Provides [`DiffWidget`] as the top-level [`StatefulWidget`] for rendering
//! a [`FileDiffMetadata`] in unified or split layout. Uses the diff iterator,
//! syntax highlighter, and inline diff modules to produce fully styled output.

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{StatefulWidget, Widget};

use crate::highlight::SyntaxHighlighter;
use crate::inline_diff::{InlineSpan, compute_inline_diff};
use crate::iter::{
    DiffLineEvent, DiffStyle, HunkExpansionRegion, IterateOverDiffProps, iterate_over_diff,
};
use crate::{ChangeType, FileDiffMetadata, LineDiffType};

// --- Colors ---

const DELETION_BG: Color = Color::Rgb(60, 20, 20);
const ADDITION_BG: Color = Color::Rgb(20, 60, 20);
const DELETION_INLINE_BG: Color = Color::Rgb(120, 30, 30);
const ADDITION_INLINE_BG: Color = Color::Rgb(30, 120, 30);
const CONTEXT_FG: Color = Color::DarkGray;
const GUTTER_FG: Color = Color::DarkGray;
const SEPARATOR_FG: Color = Color::Cyan;
const HEADER_FG: Color = Color::White;
const STATS_ADD_FG: Color = Color::Green;
const STATS_DEL_FG: Color = Color::Red;

// --- DiffViewState ---

/// Scrolling and navigation state for a diff view.
#[derive(Debug, Clone, Default)]
pub struct DiffViewState {
    /// Current scroll offset in lines.
    pub scroll_offset: usize,
    /// Currently selected hunk index (for hunk-to-hunk navigation).
    pub selected_hunk: Option<usize>,
    /// Viewport height in lines (updated during render).
    pub viewport_height: usize,
}

impl DiffViewState {
    /// Scroll up by `n` lines.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down by `n` lines, clamped to `max_lines`.
    pub fn scroll_down(&mut self, n: usize, max_lines: usize) {
        self.scroll_offset = (self.scroll_offset + n).min(max_lines.saturating_sub(1));
    }

    /// Scroll up by one page (viewport height).
    pub fn page_up(&mut self) {
        let page = self.viewport_height.max(1);
        self.scroll_up(page);
    }

    /// Scroll down by one page, clamped to `max_lines`.
    pub fn page_down(&mut self, max_lines: usize) {
        let page = self.viewport_height.max(1);
        self.scroll_down(page, max_lines);
    }

    /// Jump to the next hunk.
    pub fn next_hunk(&mut self, diff: &FileDiffMetadata, style: DiffStyle) {
        let total = diff.hunks.len();
        if total == 0 {
            return;
        }
        let next = match self.selected_hunk {
            Some(i) if i + 1 < total => i + 1,
            Some(_) => return,
            None => 0,
        };
        self.selected_hunk = Some(next);
        self.scroll_offset = match style {
            DiffStyle::Split => diff.hunks[next].split_line_start,
            _ => diff.hunks[next].unified_line_start,
        };
    }

    /// Jump to the previous hunk.
    pub fn prev_hunk(&mut self, diff: &FileDiffMetadata, style: DiffStyle) {
        if diff.hunks.is_empty() {
            return;
        }
        let prev = match self.selected_hunk {
            Some(i) if i > 0 => i - 1,
            Some(_) => return,
            None => diff.hunks.len() - 1,
        };
        self.selected_hunk = Some(prev);
        self.scroll_offset = match style {
            DiffStyle::Split => diff.hunks[prev].split_line_start,
            _ => diff.hunks[prev].unified_line_start,
        };
    }
}

// --- FileHeader ---

/// Widget that renders a file diff header.
///
/// ```text
/// ── src/main.rs (Modified) ── +15 / -8 ──
/// ```
pub struct FileHeader<'a> {
    diff: &'a FileDiffMetadata,
}

impl<'a> FileHeader<'a> {
    pub fn new(diff: &'a FileDiffMetadata) -> Self {
        Self { diff }
    }

    fn change_type_label(&self) -> &'static str {
        match self.diff.change_type {
            ChangeType::Change => "Modified",
            ChangeType::New => "Added",
            ChangeType::Deleted => "Deleted",
            ChangeType::RenamePure => "Renamed",
            ChangeType::RenameChanged => "Renamed+Modified",
        }
    }

    fn compute_stats(&self) -> (usize, usize) {
        let mut additions = 0;
        let mut deletions = 0;
        for hunk in &self.diff.hunks {
            additions += hunk.addition_lines;
            deletions += hunk.deletion_lines;
        }
        (additions, deletions)
    }
}

impl Widget for FileHeader<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let (additions, deletions) = self.compute_stats();
        let label = self.change_type_label();
        let name = &self.diff.name;
        let prev = self.diff.prev_name.as_deref();

        let mut spans = vec![
            Span::styled("── ", Style::default().fg(CONTEXT_FG)),
            Span::styled(
                name.to_string(),
                Style::default().fg(HEADER_FG).add_modifier(Modifier::BOLD),
            ),
        ];

        if let Some(prev_name) = prev {
            spans.push(Span::styled(
                format!(" (from {prev_name})"),
                Style::default().fg(CONTEXT_FG),
            ));
        }

        spans.extend([
            Span::styled(format!(" ({label}) "), Style::default().fg(CONTEXT_FG)),
            Span::styled("── ", Style::default().fg(CONTEXT_FG)),
            Span::styled(format!("+{additions}"), Style::default().fg(STATS_ADD_FG)),
            Span::styled(" / ", Style::default().fg(CONTEXT_FG)),
            Span::styled(format!("-{deletions}"), Style::default().fg(STATS_DEL_FG)),
            Span::styled(" ──", Style::default().fg(CONTEXT_FG)),
        ]);

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);
    }
}

// --- HunkSeparator ---

/// Widget that renders a hunk separator with collapsed line indicator.
///
/// ```text
/// @@ -10,5 +12,7 @@ fn process_data
///   ··· 15 lines hidden ···
/// ```
pub struct HunkSeparator<'a> {
    /// The hunk specs string (e.g. `@@ -10,5 +12,7 @@`).
    pub specs: Option<&'a str>,
    /// The hunk context (function name after @@).
    pub context: Option<&'a str>,
    /// Number of collapsed (hidden) lines.
    pub collapsed_lines: usize,
}

impl<'a> HunkSeparator<'a> {
    pub fn new(collapsed_lines: usize) -> Self {
        Self {
            specs: None,
            context: None,
            collapsed_lines,
        }
    }

    pub fn with_specs(mut self, specs: &'a str) -> Self {
        self.specs = Some(specs);
        self
    }

    pub fn with_context(mut self, context: &'a str) -> Self {
        self.context = Some(context);
        self
    }
}

impl Widget for HunkSeparator<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let mut spans = Vec::new();

        if let Some(specs) = self.specs {
            spans.push(Span::styled(
                specs.to_string(),
                Style::default().fg(SEPARATOR_FG),
            ));
            if let Some(ctx) = self.context {
                spans.push(Span::styled(
                    format!(" {ctx}"),
                    Style::default().fg(CONTEXT_FG),
                ));
            }
        }

        if self.collapsed_lines > 0 {
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                format!("··· {} lines hidden ···", self.collapsed_lines),
                Style::default().fg(CONTEXT_FG).add_modifier(Modifier::DIM),
            ));
        }

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);
    }
}

// --- DiffWidget ---

/// Top-level widget for rendering a file diff.
///
/// Implements [`StatefulWidget`] with [`DiffViewState`] to support scrolling
/// and hunk navigation. Renders either unified or split view.
pub struct DiffWidget<'a> {
    diff: &'a FileDiffMetadata,
    style: DiffStyle,
    highlighter: Option<&'a SyntaxHighlighter>,
    inline_diff_mode: LineDiffType,
    show_line_numbers: bool,
    show_file_header: bool,
    expanded_hunks: Option<&'a HashMap<usize, HunkExpansionRegion>>,
    expand_all: bool,
}

impl<'a> DiffWidget<'a> {
    pub fn new(diff: &'a FileDiffMetadata, style: DiffStyle) -> Self {
        Self {
            diff,
            style,
            highlighter: None,
            inline_diff_mode: LineDiffType::WordAlt,
            show_line_numbers: true,
            show_file_header: true,
            expanded_hunks: None,
            expand_all: false,
        }
    }

    pub fn highlighter(mut self, hl: &'a SyntaxHighlighter) -> Self {
        self.highlighter = Some(hl);
        self
    }

    pub fn inline_diff_mode(mut self, mode: LineDiffType) -> Self {
        self.inline_diff_mode = mode;
        self
    }

    pub fn show_line_numbers(mut self, show: bool) -> Self {
        self.show_line_numbers = show;
        self
    }

    pub fn show_file_header(mut self, show: bool) -> Self {
        self.show_file_header = show;
        self
    }

    pub fn expanded_hunks(mut self, map: &'a HashMap<usize, HunkExpansionRegion>) -> Self {
        self.expanded_hunks = Some(map);
        self
    }

    pub fn expand_all(mut self, expand: bool) -> Self {
        self.expand_all = expand;
        self
    }
}

impl StatefulWidget for DiffWidget<'_> {
    type State = DiffViewState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let mut y = area.y;
        let max_y = area.y + area.height;

        // Render file header
        if self.show_file_header && y < max_y {
            let header_area = Rect::new(area.x, y, area.width, 1);
            FileHeader::new(self.diff).render(header_area, buf);
            y += 1;
        }

        let content_height = (max_y - y) as usize;
        state.viewport_height = content_height;

        if content_height == 0 {
            return;
        }

        match self.style {
            DiffStyle::Split => {
                self.render_split(area.x, y, area.width, content_height, buf, state);
            }
            _ => {
                self.render_unified(area.x, y, area.width, content_height, buf, state);
            }
        }
    }
}

impl DiffWidget<'_> {
    fn render_unified(
        &self,
        x: u16,
        start_y: u16,
        width: u16,
        height: usize,
        buf: &mut Buffer,
        state: &DiffViewState,
    ) {
        let gutter_width: u16 = if self.show_line_numbers { 10 } else { 0 };
        let content_width = width.saturating_sub(gutter_width);

        let iter_style = DiffStyle::Unified;
        let mut props = IterateOverDiffProps::new(self.diff, iter_style);
        props.starting_line = state.scroll_offset;
        props.total_lines = height;
        props.expanded_hunks = self.expanded_hunks;
        props.expand_all = self.expand_all;

        let mut line_y: u16 = 0;

        iterate_over_diff(&props, |event| {
            if line_y as usize >= height {
                return true;
            }

            let y = start_y + line_y;
            let mut rendered_separator = false;

            // Check for hunk separator (collapsed_before > 0)
            let collapsed_before = match &event {
                DiffLineEvent::Context {
                    collapsed_before, ..
                }
                | DiffLineEvent::ContextExpanded {
                    collapsed_before, ..
                }
                | DiffLineEvent::Change {
                    collapsed_before, ..
                } => *collapsed_before,
            };

            if collapsed_before > 0 && (line_y as usize) < height {
                // Find the hunk to get specs/context
                let hunk_index = match &event {
                    DiffLineEvent::Context { hunk_index, .. }
                    | DiffLineEvent::ContextExpanded { hunk_index, .. }
                    | DiffLineEvent::Change { hunk_index, .. } => *hunk_index,
                };
                let hunk = self.diff.hunks.get(hunk_index);
                let sep_area = Rect::new(x, y, width, 1);
                let mut sep = HunkSeparator::new(collapsed_before);
                if let Some(h) = hunk {
                    if let Some(specs) = h.hunk_specs.as_deref() {
                        sep = sep.with_specs(specs);
                    }
                    if let Some(ctx) = h.hunk_context.as_deref() {
                        sep = sep.with_context(ctx);
                    }
                }
                sep.render(sep_area, buf);
                line_y += 1;
                rendered_separator = true;

                if line_y as usize >= height {
                    return true;
                }
            }

            let current_y = if rendered_separator {
                start_y + line_y
            } else {
                y
            };

            match &event {
                DiffLineEvent::Context {
                    deletion_line,
                    addition_line,
                    ..
                } => {
                    // Gutter: show both line numbers
                    if self.show_line_numbers {
                        let gutter = format!(
                            "{:>4} {:>4} ",
                            deletion_line.line_number, addition_line.line_number
                        );
                        buf.set_string(x, current_y, &gutter, Style::default().fg(GUTTER_FG));
                    }

                    // Content
                    let line_text = self
                        .diff
                        .addition_lines
                        .get(addition_line.line_index)
                        .map(|s| s.as_str())
                        .unwrap_or("");

                    let spans = self.highlight_line(line_text);
                    let line = Line::from(spans);
                    buf.set_line(x + gutter_width, current_y, &line, content_width);
                }
                DiffLineEvent::ContextExpanded {
                    deletion_line,
                    addition_line,
                    ..
                } => {
                    if self.show_line_numbers {
                        let gutter = format!(
                            "{:>4} {:>4} ",
                            deletion_line.line_number, addition_line.line_number
                        );
                        buf.set_string(
                            x,
                            current_y,
                            &gutter,
                            Style::default().fg(GUTTER_FG).add_modifier(Modifier::DIM),
                        );
                    }

                    let line_text = self
                        .diff
                        .addition_lines
                        .get(addition_line.line_index)
                        .map(|s| s.as_str())
                        .unwrap_or("");

                    let mut spans = self.highlight_line(line_text);
                    for span in &mut spans {
                        span.style = span.style.add_modifier(Modifier::DIM);
                    }
                    let line = Line::from(spans);
                    buf.set_line(x + gutter_width, current_y, &line, content_width);
                }
                DiffLineEvent::Change {
                    deletion_line,
                    addition_line,
                    ..
                } => {
                    let (is_deletion, line_num, line_idx, bg, inline_bg) =
                        if let Some(del) = deletion_line {
                            (
                                true,
                                del.line_number,
                                del.line_index,
                                DELETION_BG,
                                DELETION_INLINE_BG,
                            )
                        } else if let Some(add) = addition_line {
                            (
                                false,
                                add.line_number,
                                add.line_index,
                                ADDITION_BG,
                                ADDITION_INLINE_BG,
                            )
                        } else {
                            line_y += 1;
                            return false;
                        };

                    // Fill background
                    let bg_style = Style::default().bg(bg);
                    for col in x..(x + width).min(buf.area().width) {
                        buf[(col, current_y)].set_style(bg_style);
                    }

                    // Gutter
                    if self.show_line_numbers {
                        let (del_str, add_str) = if is_deletion {
                            (format!("{:>4}", line_num), "     ".to_string())
                        } else {
                            ("     ".to_string(), format!("{:>4}", line_num))
                        };
                        let prefix = if is_deletion { "-" } else { "+" };
                        let gutter = format!("{del_str} {add_str}{prefix}");
                        buf.set_string(
                            x,
                            current_y,
                            &gutter,
                            Style::default().fg(GUTTER_FG).bg(bg),
                        );
                    }

                    // Content with optional inline diffs
                    let lines_arr = if is_deletion {
                        &self.diff.deletion_lines
                    } else {
                        &self.diff.addition_lines
                    };
                    let line_text = lines_arr.get(line_idx).map(|s| s.as_str()).unwrap_or("");

                    let spans = self.render_change_line(
                        line_text,
                        is_deletion,
                        deletion_line.as_ref(),
                        addition_line.as_ref(),
                        bg,
                        inline_bg,
                    );
                    let line = Line::from(spans);
                    buf.set_line(x + gutter_width, current_y, &line, content_width);
                }
            }

            line_y += 1;
            false
        });
    }

    fn render_split(
        &self,
        x: u16,
        start_y: u16,
        width: u16,
        height: usize,
        buf: &mut Buffer,
        state: &DiffViewState,
    ) {
        let half_width = width / 2;
        let left_area = Rect::new(x, start_y, half_width, height as u16);
        let right_area = Rect::new(x + half_width, start_y, width - half_width, height as u16);
        let gutter_width: u16 = if self.show_line_numbers { 6 } else { 0 };
        let content_width_left = left_area.width.saturating_sub(gutter_width);
        let content_width_right = right_area.width.saturating_sub(gutter_width);

        // Draw divider
        for row in start_y..start_y + height as u16 {
            if half_width > 0 && x + half_width - 1 < buf.area().width {
                buf[(x + half_width - 1, row)]
                    .set_char('│')
                    .set_style(Style::default().fg(CONTEXT_FG));
            }
        }

        let iter_style = DiffStyle::Split;
        let mut props = IterateOverDiffProps::new(self.diff, iter_style);
        props.starting_line = state.scroll_offset;
        props.total_lines = height;
        props.expanded_hunks = self.expanded_hunks;
        props.expand_all = self.expand_all;

        let mut line_y: u16 = 0;

        iterate_over_diff(&props, |event| {
            if line_y as usize >= height {
                return true;
            }

            let current_y = start_y + line_y;

            // Check for separator
            let collapsed_before = match &event {
                DiffLineEvent::Context {
                    collapsed_before, ..
                }
                | DiffLineEvent::ContextExpanded {
                    collapsed_before, ..
                }
                | DiffLineEvent::Change {
                    collapsed_before, ..
                } => *collapsed_before,
            };

            if collapsed_before > 0 {
                let hunk_index = match &event {
                    DiffLineEvent::Context { hunk_index, .. }
                    | DiffLineEvent::ContextExpanded { hunk_index, .. }
                    | DiffLineEvent::Change { hunk_index, .. } => *hunk_index,
                };
                let hunk = self.diff.hunks.get(hunk_index);
                let sep_area = Rect::new(x, current_y, width, 1);
                let mut sep = HunkSeparator::new(collapsed_before);
                if let Some(h) = hunk {
                    if let Some(specs) = h.hunk_specs.as_deref() {
                        sep = sep.with_specs(specs);
                    }
                    if let Some(ctx) = h.hunk_context.as_deref() {
                        sep = sep.with_context(ctx);
                    }
                }
                sep.render(sep_area, buf);
                line_y += 1;
                if line_y as usize >= height {
                    return true;
                }
            }

            let current_y = start_y + line_y;

            match &event {
                DiffLineEvent::Context {
                    deletion_line,
                    addition_line,
                    ..
                } => {
                    // Left side (deletion/old)
                    if self.show_line_numbers {
                        let gutter = format!("{:>4} ", deletion_line.line_number);
                        buf.set_string(
                            left_area.x,
                            current_y,
                            &gutter,
                            Style::default().fg(GUTTER_FG),
                        );
                    }
                    let line_text = self
                        .diff
                        .deletion_lines
                        .get(deletion_line.line_index)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let spans = self.highlight_line(line_text);
                    buf.set_line(
                        left_area.x + gutter_width,
                        current_y,
                        &Line::from(spans),
                        content_width_left,
                    );

                    // Right side (addition/new)
                    if self.show_line_numbers {
                        let gutter = format!("{:>4} ", addition_line.line_number);
                        buf.set_string(
                            right_area.x,
                            current_y,
                            &gutter,
                            Style::default().fg(GUTTER_FG),
                        );
                    }
                    let line_text = self
                        .diff
                        .addition_lines
                        .get(addition_line.line_index)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let spans = self.highlight_line(line_text);
                    buf.set_line(
                        right_area.x + gutter_width,
                        current_y,
                        &Line::from(spans),
                        content_width_right,
                    );
                }
                DiffLineEvent::ContextExpanded {
                    deletion_line,
                    addition_line,
                    ..
                } => {
                    let dim_style = Style::default().fg(GUTTER_FG).add_modifier(Modifier::DIM);

                    if self.show_line_numbers {
                        buf.set_string(
                            left_area.x,
                            current_y,
                            format!("{:>4} ", deletion_line.line_number),
                            dim_style,
                        );
                    }
                    let line_text = self
                        .diff
                        .deletion_lines
                        .get(deletion_line.line_index)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let mut spans = self.highlight_line(line_text);
                    for s in &mut spans {
                        s.style = s.style.add_modifier(Modifier::DIM);
                    }
                    buf.set_line(
                        left_area.x + gutter_width,
                        current_y,
                        &Line::from(spans),
                        content_width_left,
                    );

                    if self.show_line_numbers {
                        buf.set_string(
                            right_area.x,
                            current_y,
                            format!("{:>4} ", addition_line.line_number),
                            dim_style,
                        );
                    }
                    let line_text = self
                        .diff
                        .addition_lines
                        .get(addition_line.line_index)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let mut spans = self.highlight_line(line_text);
                    for s in &mut spans {
                        s.style = s.style.add_modifier(Modifier::DIM);
                    }
                    buf.set_line(
                        right_area.x + gutter_width,
                        current_y,
                        &Line::from(spans),
                        content_width_right,
                    );
                }
                DiffLineEvent::Change {
                    deletion_line,
                    addition_line,
                    ..
                } => {
                    // Left side: deletion
                    if let Some(del) = deletion_line {
                        // Fill left background
                        for col in left_area.x..left_area.x + left_area.width - 1 {
                            buf[(col, current_y)].set_style(Style::default().bg(DELETION_BG));
                        }
                        if self.show_line_numbers {
                            let gutter = format!("{:>4}-", del.line_number);
                            buf.set_string(
                                left_area.x,
                                current_y,
                                &gutter,
                                Style::default().fg(GUTTER_FG).bg(DELETION_BG),
                            );
                        }
                        let line_text = self
                            .diff
                            .deletion_lines
                            .get(del.line_index)
                            .map(|s| s.as_str())
                            .unwrap_or("");

                        let spans = self.render_change_line(
                            line_text,
                            true,
                            deletion_line.as_ref(),
                            addition_line.as_ref(),
                            DELETION_BG,
                            DELETION_INLINE_BG,
                        );
                        buf.set_line(
                            left_area.x + gutter_width,
                            current_y,
                            &Line::from(spans),
                            content_width_left,
                        );
                    }

                    // Right side: addition
                    if let Some(add) = addition_line {
                        for col in right_area.x..right_area.x + right_area.width {
                            buf[(col, current_y)].set_style(Style::default().bg(ADDITION_BG));
                        }
                        if self.show_line_numbers {
                            let gutter = format!("{:>4}+", add.line_number);
                            buf.set_string(
                                right_area.x,
                                current_y,
                                &gutter,
                                Style::default().fg(GUTTER_FG).bg(ADDITION_BG),
                            );
                        }
                        let line_text = self
                            .diff
                            .addition_lines
                            .get(add.line_index)
                            .map(|s| s.as_str())
                            .unwrap_or("");

                        let spans = self.render_change_line(
                            line_text,
                            false,
                            deletion_line.as_ref(),
                            addition_line.as_ref(),
                            ADDITION_BG,
                            ADDITION_INLINE_BG,
                        );
                        buf.set_line(
                            right_area.x + gutter_width,
                            current_y,
                            &Line::from(spans),
                            content_width_right,
                        );
                    }
                }
            }

            line_y += 1;
            false
        });
    }

    /// Highlight a line using the syntax highlighter (if available) or return plain.
    fn highlight_line(&self, text: &str) -> Vec<Span<'static>> {
        match self.highlighter {
            Some(hl) => hl.highlight_line(text, &self.diff.name),
            None => vec![Span::raw(text.to_owned())],
        }
    }

    /// Render a changed line with optional inline diff highlighting.
    fn render_change_line(
        &self,
        line_text: &str,
        is_deletion: bool,
        deletion_line: Option<&crate::iter::DiffLineMetadata>,
        addition_line: Option<&crate::iter::DiffLineMetadata>,
        bg: Color,
        inline_bg: Color,
    ) -> Vec<Span<'static>> {
        // Try to compute inline diffs if both sides are available
        let inline_spans = if self.inline_diff_mode != LineDiffType::None {
            if let (Some(del), Some(add)) = (deletion_line, addition_line) {
                let old_text = self
                    .diff
                    .deletion_lines
                    .get(del.line_index)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let new_text = self
                    .diff
                    .addition_lines
                    .get(add.line_index)
                    .map(|s| s.as_str())
                    .unwrap_or("");

                let (del_spans, add_spans) =
                    compute_inline_diff(old_text, new_text, self.inline_diff_mode);

                if is_deletion {
                    Some(del_spans)
                } else {
                    Some(add_spans)
                }
            } else {
                None
            }
        } else {
            None
        };

        match inline_spans {
            Some(spans) if !spans.is_empty() => {
                // Render with inline diff highlighting
                render_inline_spans(&spans, bg, inline_bg)
            }
            _ => {
                // Plain syntax-highlighted line with diff background
                let mut spans = self.highlight_line(line_text);
                for span in &mut spans {
                    span.style = span.style.bg(bg);
                }
                spans
            }
        }
    }
}

/// Convert InlineSpans to ratatui Spans with appropriate styling.
fn render_inline_spans(
    spans: &[InlineSpan],
    base_bg: Color,
    highlight_bg: Color,
) -> Vec<Span<'static>> {
    spans
        .iter()
        .map(|span| {
            let bg = if span.highlighted {
                highlight_bg
            } else {
                base_bg
            };
            let mut style = Style::default().bg(bg);
            if span.highlighted {
                style = style.add_modifier(Modifier::BOLD);
            }
            Span::styled(span.text.clone(), style)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn test_diff() -> FileDiffMetadata {
        FileDiffMetadata {
            name: "src/main.rs".into(),
            prev_name: None,
            change_type: ChangeType::Change,
            hunks: vec![Hunk {
                collapsed_before: 0,
                addition_start: 1,
                addition_count: 4,
                addition_lines: 1,
                addition_line_index: 0,
                deletion_start: 1,
                deletion_count: 4,
                deletion_lines: 1,
                deletion_line_index: 0,
                hunk_content: vec![
                    HunkContent::Context(ContextContent {
                        lines: 1,
                        addition_line_index: 0,
                        deletion_line_index: 0,
                    }),
                    HunkContent::Change(ChangeContent {
                        deletions: 1,
                        deletion_line_index: 1,
                        additions: 1,
                        addition_line_index: 1,
                    }),
                    HunkContent::Context(ContextContent {
                        lines: 2,
                        addition_line_index: 2,
                        deletion_line_index: 2,
                    }),
                ],
                hunk_context: None,
                hunk_specs: Some("@@ -1,4 +1,4 @@".into()),
                split_line_start: 0,
                split_line_count: 4,
                unified_line_start: 0,
                unified_line_count: 5,
                no_eof_cr_deletions: false,
                no_eof_cr_additions: false,
            }],
            split_line_count: 4,
            unified_line_count: 5,
            is_partial: true,
            deletion_lines: vec![
                "fn main() {".into(),
                "    let x = 1;".into(),
                "    println!(\"hello\");".into(),
                "}".into(),
            ],
            addition_lines: vec![
                "fn main() {".into(),
                "    let x = 2;".into(),
                "    println!(\"hello\");".into(),
                "}".into(),
            ],
        }
    }

    #[test]
    fn file_header_renders() {
        let diff = test_diff();
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        FileHeader::new(&diff).render(area, &mut buf);

        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            content.contains("src/main.rs"),
            "header should contain filename"
        );

        assert!(content.contains("+1"), "header should show additions");
        assert!(content.contains("-1"), "header should show deletions");
    }

    #[test]
    fn hunk_separator_renders() {
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        HunkSeparator::new(15)
            .with_specs("@@ -10,5 +12,7 @@")
            .with_context("fn process_data")
            .render(area, &mut buf);

        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("@@ -10,5 +12,7 @@"));
        assert!(content.contains("15"));
    }

    #[test]
    fn unified_diff_renders_to_buffer() {
        let diff = test_diff();
        // 1 header + 5 unified lines
        let area = Rect::new(0, 0, 60, 7);
        let mut buf = Buffer::empty(area);
        let mut state = DiffViewState::default();

        DiffWidget::new(&diff, DiffStyle::Unified)
            .show_line_numbers(true)
            .show_file_header(true)
            .render(area, &mut buf, &mut state);

        // Verify viewport_height was set
        assert_eq!(state.viewport_height, 6); // 7 - 1 header

        // Check that something was rendered (non-empty buffer)
        let has_content = buf.content().iter().any(|c| c.symbol() != " ");
        assert!(has_content, "buffer should have content");
    }

    #[test]
    fn split_diff_divides_area() {
        let diff = test_diff();
        let area = Rect::new(0, 0, 80, 6);
        let mut buf = Buffer::empty(area);
        let mut state = DiffViewState::default();

        DiffWidget::new(&diff, DiffStyle::Split)
            .show_line_numbers(true)
            .show_file_header(true)
            .render(area, &mut buf, &mut state);

        // Check the divider character exists at the midpoint
        let mid = 39; // half_width - 1 = 40 - 1
        let has_divider = (1..6).any(|row| {
            let cell = &buf[(mid, row)];
            cell.symbol() == "│"
        });
        assert!(has_divider, "split view should have vertical divider");
    }

    #[test]
    fn scroll_state_navigation() {
        let mut state = DiffViewState {
            scroll_offset: 5,
            selected_hunk: None,
            viewport_height: 10,
        };

        state.scroll_up(3);
        assert_eq!(state.scroll_offset, 2);

        state.scroll_up(10);
        assert_eq!(state.scroll_offset, 0, "should not go below 0");

        state.scroll_down(15, 20);
        assert_eq!(state.scroll_offset, 15);

        state.scroll_down(10, 20);
        assert_eq!(state.scroll_offset, 19, "should clamp to max-1");
    }

    #[test]
    fn hunk_navigation() {
        let diff = test_diff();
        let mut state = DiffViewState::default();

        state.next_hunk(&diff, DiffStyle::Unified);
        assert_eq!(state.selected_hunk, Some(0));
        assert_eq!(state.scroll_offset, 0);

        // Only 1 hunk, so next should not advance
        state.next_hunk(&diff, DiffStyle::Unified);
        assert_eq!(state.selected_hunk, Some(0));

        // Prev from 0 should not go below
        state.prev_hunk(&diff, DiffStyle::Unified);
        assert_eq!(state.selected_hunk, Some(0));
    }

    #[test]
    fn page_navigation() {
        let mut state = DiffViewState {
            scroll_offset: 50,
            selected_hunk: None,
            viewport_height: 20,
        };

        state.page_up();
        assert_eq!(state.scroll_offset, 30);

        state.page_down(100);
        assert_eq!(state.scroll_offset, 50);
    }

    #[test]
    fn empty_diff_renders_without_panic() {
        let diff = FileDiffMetadata {
            name: "empty.rs".into(),
            prev_name: None,
            change_type: ChangeType::Change,
            hunks: vec![],
            split_line_count: 0,
            unified_line_count: 0,
            is_partial: true,
            deletion_lines: vec![],
            addition_lines: vec![],
        };

        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        let mut state = DiffViewState::default();

        DiffWidget::new(&diff, DiffStyle::Unified).render(area, &mut buf, &mut state);
        // Should not panic
    }

    #[test]
    fn zero_area_renders_without_panic() {
        let diff = test_diff();
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        let mut state = DiffViewState::default();

        DiffWidget::new(&diff, DiffStyle::Unified).render(area, &mut buf, &mut state);
    }
}
