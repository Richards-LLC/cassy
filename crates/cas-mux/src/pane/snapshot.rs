use crate::error::{Error, Result};
use crate::pane::Pane;
use crate::pane::style::{cell_style_to_flags, convert_style_runs_to_proto, debug_log_enabled};
use cas_factory_protocol::{CacheRow, CursorPosition, RowData, TerminalCell, TerminalSnapshot};

impl Pane {
    /// Get scrollback information from the terminal
    pub fn scrollback_info(&self) -> ghostty_vt::ScrollbackInfo {
        self.terminal.scrollback_info()
    }

    /// Get the current scroll offset (lines from bottom, 0 = at bottom)
    pub fn scroll_offset(&self) -> u32 {
        self.terminal.scrollback_info().viewport_offset
    }

    /// Get the total number of scrollback lines
    pub fn scrollback_lines(&self) -> u32 {
        self.terminal.scrollback_info().total_scrollback
    }

    /// Get a full terminal snapshot for the current viewport
    ///
    /// Uses row_style_runs() (one FFI call per row) instead of the old
    /// dump_row() + row_styles() (two FFI calls per row) + per-cell iteration.
    pub fn get_full_snapshot(&self) -> Result<TerminalSnapshot> {
        let total_cells = (self.rows as usize) * (self.cols as usize);
        let mut cells = Vec::with_capacity(total_cells);

        for row in 0..self.rows {
            let text = self.dump_row(row).unwrap_or_default();
            let style_runs = self
                .terminal
                .row_style_runs(row)
                .map_err(|e| Error::terminal(e.to_string()))?;

            // Convert style runs to per-cell TerminalCell entries
            let cols = self.cols as usize;
            let chars: Vec<char> = text.chars().collect();
            let mut col = 0usize;
            for run in &style_runs {
                let start = run.start_col as usize;
                let end = (run.end_col as usize).min(cols);
                let fg = (run.style.fg.r, run.style.fg.g, run.style.fg.b);
                let bg = (run.style.bg.r, run.style.bg.g, run.style.bg.b);
                let flags = cell_style_to_flags(&run.style);

                // Fill gap before this run with default cells
                while col < start && col < cols {
                    let ch = chars.get(col).copied().unwrap_or(' ');
                    cells.push(TerminalCell {
                        codepoint: ch as u32,
                        fg: (0, 0, 0),
                        bg: (0, 0, 0),
                        flags: 0,
                        width: 1,
                    });
                    col += 1;
                }

                // Fill this run's cells
                for c in start..end {
                    if c >= cols {
                        break;
                    }
                    let ch = chars.get(c).copied().unwrap_or(' ');
                    cells.push(TerminalCell {
                        codepoint: ch as u32,
                        fg,
                        bg,
                        flags,
                        width: 1,
                    });
                    col = c + 1;
                }
            }

            // Fill remaining columns with default cells
            while col < cols {
                let ch = chars.get(col).copied().unwrap_or(' ');
                cells.push(TerminalCell {
                    codepoint: ch as u32,
                    fg: (0, 0, 0),
                    bg: (0, 0, 0),
                    flags: 0,
                    width: 1,
                });
                col += 1;
            }
        }

        let (cursor_col, cursor_row) = self.cursor_position();
        Ok(TerminalSnapshot {
            cells,
            cursor: CursorPosition {
                x: cursor_col.saturating_sub(1),
                y: cursor_row.saturating_sub(1),
            },
            cols: self.cols,
            rows: self.rows,
        })
    }

    #[allow(clippy::type_complexity)]
    pub fn get_incremental_update(
        &mut self,
    ) -> Result<Option<(Vec<RowData>, Option<CursorPosition>, u64)>> {
        let mut force_all = self.take_force_all_dirty();
        let scroll_info = self.terminal.scrollback_info();
        if scroll_info.total_scrollback != self.last_total_scrollback {
            if scroll_info.total_scrollback > self.last_total_scrollback {
                force_all = true;
            }
            self.last_total_scrollback = scroll_info.total_scrollback;
        }

        let dirty_rows = if force_all {
            if debug_log_enabled() {
                tracing::debug!(
                    "Pane {}: force_all_dirty, returning all {} rows",
                    self.id,
                    self.rows
                );
            }
            (0..self.rows).collect::<Vec<_>>()
        } else {
            let rows = self.terminal.take_dirty_rows(self.rows);
            if debug_log_enabled() && !rows.is_empty() {
                tracing::debug!("Pane {}: {} dirty rows from terminal", self.id, rows.len());
            }
            rows
        };

        if dirty_rows.is_empty() {
            return Ok(None);
        }

        let mut rows = Vec::with_capacity(dirty_rows.len());
        for row_idx in dirty_rows {
            let text = match self.dump_row(row_idx) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        "Pane {}: dump_row({}) failed (pane.rows={}): {}",
                        self.id,
                        row_idx,
                        self.rows,
                        e
                    );
                    return Err(e);
                }
            };
            let style_runs = self
                .terminal
                .row_style_runs(row_idx)
                .map_err(|e| Error::terminal(e.to_string()))?;

            let runs = convert_style_runs_to_proto(&text, &style_runs, self.cols as usize);
            rows.push(RowData { row: row_idx, runs });
        }

        let (cursor_col, cursor_row) = self.cursor_position();
        let cursor = Some(CursorPosition {
            x: cursor_col.saturating_sub(1),
            y: cursor_row.saturating_sub(1),
        });

        let seq = self.seq_counter;
        self.seq_counter = self.seq_counter.wrapping_add(1);

        Ok(Some((rows, cursor, seq)))
    }

    pub fn create_snapshot_with_cache(
        &self,
        cache_window: u32,
    ) -> Result<(TerminalSnapshot, Vec<CacheRow>, Option<u32>)> {
        let snapshot = self.get_full_snapshot()?;

        if cache_window == 0 {
            return Ok((snapshot, Vec::new(), None));
        }

        let info = self.terminal.scrollback_info();
        let viewport_rows = info.viewport_rows as u32;
        let total = info.total_scrollback;

        let viewport_bottom = total.saturating_sub(info.viewport_offset);
        let viewport_top = viewport_bottom.saturating_sub(viewport_rows);

        let buffer = cache_window.min(viewport_rows * 2);
        let cache_start = viewport_top.saturating_sub(buffer);
        let cache_end = (viewport_bottom + buffer).min(total);

        let mut cache_rows = Vec::new();
        for screen_row in cache_start..cache_end {
            if screen_row >= viewport_top && screen_row < viewport_bottom {
                continue;
            }

            let text = self
                .terminal
                .dump_screen_row(screen_row)
                .unwrap_or_default();
            let style_runs = self
                .terminal
                .screen_row_style_runs(screen_row)
                .unwrap_or_default();

            let proto_runs = convert_style_runs_to_proto(&text, &style_runs, self.cols as usize);
            cache_rows.push(CacheRow {
                screen_row,
                text,
                style_runs: proto_runs,
            });
        }

        Ok((snapshot, cache_rows, Some(cache_start)))
    }

    pub fn create_snapshot_rows_with_cache(
        &self,
        cache_window: u32,
    ) -> Result<(Vec<RowData>, Vec<CacheRow>, Option<u32>)> {
        let info = self.terminal.scrollback_info();
        let viewport_rows = info.viewport_rows as u32;
        let total = info.total_scrollback;

        let rows = self.get_viewport_rows_data()?;

        if cache_window == 0 {
            return Ok((rows, Vec::new(), None));
        }

        let viewport_bottom = total.saturating_sub(info.viewport_offset);
        let viewport_top = viewport_bottom.saturating_sub(viewport_rows);

        let buffer = cache_window.min(viewport_rows * 2);
        let cache_start = viewport_top.saturating_sub(buffer);
        let cache_end = (viewport_bottom + buffer).min(total);

        let mut cache_rows = Vec::new();
        for screen_row in cache_start..cache_end {
            if screen_row >= viewport_top && screen_row < viewport_bottom {
                continue;
            }

            let text = self
                .terminal
                .dump_screen_row(screen_row)
                .unwrap_or_default();
            let style_runs = self
                .terminal
                .screen_row_style_runs(screen_row)
                .unwrap_or_default();

            let proto_runs = convert_style_runs_to_proto(&text, &style_runs, self.cols as usize);
            cache_rows.push(CacheRow {
                screen_row,
                text,
                style_runs: proto_runs,
            });
        }

        Ok((rows, cache_rows, Some(cache_start)))
    }

    /// Return an absolute, bounded page of styled historical screen rows.
    ///
    /// The caller owns the request-size ceiling; this method never returns
    /// more than `count` rows and never changes the pane's live viewport.
    pub fn scrollback_page(&self, start_row: u32, count: u16) -> Vec<CacheRow> {
        let end_row = start_row
            .saturating_add(u32::from(count))
            .min(self.scrollback_lines());
        let mut rows = Vec::with_capacity(end_row.saturating_sub(start_row) as usize);
        for screen_row in start_row..end_row {
            let text = self
                .terminal
                .dump_screen_row(screen_row)
                .unwrap_or_default();
            let style_runs = self
                .terminal
                .screen_row_style_runs(screen_row)
                .unwrap_or_default();
            rows.push(CacheRow {
                screen_row,
                style_runs: convert_style_runs_to_proto(&text, &style_runs, self.cols as usize),
                text,
            });
        }
        rows
    }

    pub fn get_viewport_rows_data(&self) -> Result<Vec<RowData>> {
        let mut rows = Vec::with_capacity(self.rows as usize);
        for row_idx in 0..self.rows {
            let text = self.dump_row(row_idx).unwrap_or_default();
            let style_runs = self
                .terminal
                .row_style_runs(row_idx)
                .map_err(|e| Error::terminal(e.to_string()))?;
            let runs = convert_style_runs_to_proto(&text, &style_runs, self.cols as usize);
            rows.push(RowData { row: row_idx, runs });
        }
        Ok(rows)
    }
}
