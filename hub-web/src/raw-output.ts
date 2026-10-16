import { TRANSCRIPT_CELL_WIDE, type TranscriptCell, type TranscriptColor, type TranscriptRow, type TranscriptTheme } from "./transcript";
import type { TranscriptSource } from "./transcript-view";

/**
 * The Raw output drawer's text (cas-f285). Commander no longer runs a terminal
 * emulator: the supervisor's pane is read as the daemon already renders it,
 * through the bounded `ScrollbackRequest` → `ScrollbackPage` exchange
 * (protocol capability `paged_scrollback`). A daemon too old to announce its
 * panes' rows falls back to the pane's recent output bytes with their control
 * sequences removed.
 */

/** A rendered row of a pane's screen (cas-factory-protocol `CacheRow`). */
export interface ScrollbackRow {
  readonly screen_row: number;
  readonly text: string;
  readonly style_runs?: readonly ScrollbackStyleRun[];
}

/** A run of cells sharing one style (cas-factory-protocol `StyleRun`). */
export interface ScrollbackStyleRun {
  readonly text: string;
  readonly fg: readonly [number, number, number];
  readonly bg: readonly [number, number, number];
  readonly flags: number;
}

/** One page of rows answering a `ScrollbackRequest`. */
export interface ScrollbackPage {
  readonly pane_id: string;
  readonly generation: number;
  readonly start_row: number;
  readonly next_row?: number | null;
  readonly rows: readonly ScrollbackRow[];
}

/** Per-pane attach metadata from the daemon's Welcome; no terminal content. */
export interface PaneBootstrap {
  readonly pane_id: string;
  readonly epoch: number;
  readonly cols: number;
  readonly rows: number;
  readonly scrollback_start_row: number;
  readonly scrollback_end_row: number;
}

/** The daemon answers at most this many rows per request (ws_client.rs COMMANDER_SCROLLBACK_MAX_ROWS). */
export const SCROLLBACK_PAGE_ROWS = 200;
/** A page not answered in this long no longer blocks the next read. */
const PAGE_WAIT_MS = 5_000;
/** The drawer keeps at most this many of the pane's newest rows. */
export const RAW_OUTPUT_ROWS = 600;
/** The legacy byte fallback keeps at most this much of the pane's newest output. */
export const RAW_OUTPUT_BYTES = 256 * 1024;

/** The drawer's reading colours: the daemon marks the default foreground as (0,0,0). */
export const RAW_OUTPUT_THEME: TranscriptTheme = {
  foreground: { r: 201, g: 209, b: 217 },
  background: { r: 13, g: 17, b: 23 },
};

// cas-factory-protocol STYLE_* flags.
const STYLE_BOLD = 1 << 0;
const STYLE_ITALIC = 1 << 1;
const STYLE_UNDERLINE = 1 << 2;
const STYLE_STRIKETHROUGH = 1 << 3;
const STYLE_INVISIBLE = 1 << 5;

function cell(text: string, foreground: TranscriptColor, flags = 0, theme = RAW_OUTPUT_THEME): TranscriptCell {
  return {
    text,
    wide: TRANSCRIPT_CELL_WIDE.narrow,
    foreground,
    background: theme.background,
    bold: (flags & STYLE_BOLD) !== 0,
    italic: (flags & STYLE_ITALIC) !== 0,
    invisible: (flags & STYLE_INVISIBLE) !== 0,
    strikethrough: (flags & STYLE_STRIKETHROUGH) !== 0,
    underline: (flags & STYLE_UNDERLINE) !== 0,
  };
}

function textRow(text: string, theme = RAW_OUTPUT_THEME): TranscriptRow {
  return { cells: text ? [cell(text, theme.foreground, 0, theme)] : [], text, isWrapContinuation: false, wrapsToNext: false };
}

/** A daemon-rendered row as transcript cells; its style runs keep their colour and weight. */
export function rowFromScrollback(row: ScrollbackRow, theme: TranscriptTheme = RAW_OUTPUT_THEME): TranscriptRow {
  const runs = row.style_runs ?? [];
  if (runs.length === 0) return textRow(row.text, theme);
  const cells = runs.filter((run) => run.text.length > 0).map((run) => {
    const [r, g, b] = run.fg;
    // The daemon writes the terminal's default foreground as black.
    const foreground = r === 0 && g === 0 && b === 0 ? theme.foreground : { r, g, b };
    return cell(run.text, foreground, run.flags, theme);
  });
  return { cells, text: runs.map((run) => run.text).join(""), isWrapContinuation: false, wrapsToNext: false };
}

// OSC (to BEL or ST), DCS/SOS/PM/APC strings, CSI, then any other escape pair.
const CONTROL_SEQUENCE = /\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)|\u001b[PX^_][^\u001b]*\u001b\\|\u001b\[[0-?]*[ -/]*[@-~]|\u001b[@-Z\\-_]|\u001b./gu;
// C0 controls other than tab, line feed and carriage return; and DEL.
const OTHER_CONTROLS = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/gu;

/**
 * Readable lines from raw pane output, for daemons that predate paged rows:
 * control sequences are removed, and a carriage return redraws its line, so
 * a progress bar reads as its last state. Cursor-addressed screens stay
 * approximate here; that is what the daemon's rendered rows are for.
 */
export function rowsFromOutputBytes(bytes: Uint8Array, limit = RAW_OUTPUT_ROWS, theme: TranscriptTheme = RAW_OUTPUT_THEME): TranscriptRow[] {
  const text = new TextDecoder().decode(bytes).replace(CONTROL_SEQUENCE, "").replace(OTHER_CONTROLS, "");
  const lines = text.split("\n").map((line) => {
    const visible = line.replace(/\r+$/u, "");
    const redraw = visible.lastIndexOf("\r");
    return (redraw === -1 ? visible : visible.slice(redraw + 1)).replace(/\t/gu, "    ");
  });
  if (lines.at(-1) === "") lines.pop();
  return lines.slice(-limit).map((line) => textRow(line, theme));
}

/** Nothing pages in the drawer: it holds a bounded tail and re-reads it. */
abstract class BoundedSource implements TranscriptSource {
  abstract rows(): readonly TranscriptRow[];
  theme(): TranscriptTheme { return RAW_OUTPUT_THEME; }
  hasScrollbackAbove(): boolean { return false; }
  scrollRows(): void { /* bounded: nothing above the tail to page in */ }
  scrollToBottom(): void { /* the view pins its own tail */ }
}

/**
 * The newest rows of a pane, read page by page from the daemon. `refresh`
 * re-reads the tail (a TUI redraws rows in place, and new rows extend it);
 * one request is in flight at a time, and a refresh asked for meanwhile runs
 * when it lands.
 */
export class ScrollbackTail extends BoundedSource {
  private readonly byRow = new Map<number, TranscriptRow>();
  private end: number;
  private inFlight: number | undefined;
  private inFlightAt = 0;
  private refreshOwed = false;

  constructor(
    readonly paneId: string,
    readonly generation: number,
    endRow: number,
    private readonly request: (startRow: number, count: number) => boolean,
    private readonly limit = RAW_OUTPUT_ROWS,
  ) {
    super();
    this.end = Math.max(0, endRow);
  }

  /** Reads the newest `limit` rows. */
  open(): void { this.read(Math.max(0, this.end - this.limit)); }

  /** Re-reads the last page of rows and anything after it. */
  refresh(now = Date.now()): void {
    // A page that never came back (a dropped stream) stops holding the tail.
    if (this.inFlight !== undefined && now - this.inFlightAt < PAGE_WAIT_MS) { this.refreshOwed = true; return; }
    this.read(Math.max(0, this.end - SCROLLBACK_PAGE_ROWS));
  }

  /** Takes a page for this pane and generation; returns whether it was this tail's. */
  accept(page: ScrollbackPage): boolean {
    if (page.pane_id !== this.paneId || page.generation !== this.generation || page.start_row !== this.inFlight) return false;
    this.inFlight = undefined;
    for (const row of page.rows) this.byRow.set(row.screen_row, rowFromScrollback(row));
    const last = page.rows.at(-1)?.screen_row;
    if (page.rows.length >= SCROLLBACK_PAGE_ROWS && last !== undefined) {
      // A full page: more rows follow it.
      this.end = Math.max(this.end, last + 1);
      this.read(page.next_row ?? last + 1);
      return true;
    }
    // A short page ends at the pane's newest row.
    this.end = last === undefined ? Math.min(this.end, page.start_row) : last + 1;
    for (const row of [...this.byRow.keys()]) if (row >= this.end || row < this.end - this.limit) this.byRow.delete(row);
    if (this.refreshOwed) { this.refreshOwed = false; this.refresh(); }
    return true;
  }

  rows(): readonly TranscriptRow[] {
    return [...this.byRow.entries()].sort(([left], [right]) => left - right).map(([, row]) => row);
  }

  private read(startRow: number): void {
    if (!this.request(startRow, SCROLLBACK_PAGE_ROWS)) { this.inFlight = undefined; return; }
    this.inFlight = startRow;
    this.inFlightAt = Date.now();
  }
}

/** The pane's recent output bytes, for daemons without paged rows. */
export class OutputBytesSource extends BoundedSource {
  private cached: { bytes: Uint8Array; rows: readonly TranscriptRow[] } | undefined;

  constructor(private readonly bytes: () => Uint8Array | undefined) { super(); }

  rows(): readonly TranscriptRow[] {
    const bytes = this.bytes();
    if (!bytes) return [];
    if (this.cached?.bytes !== bytes) this.cached = { bytes, rows: rowsFromOutputBytes(bytes) };
    return this.cached.rows;
  }
}

/** Appends output to a pane's bounded byte tail, returning the new tail. */
export function appendOutputTail(previous: Uint8Array | undefined, data: Uint8Array, limit = RAW_OUTPUT_BYTES): Uint8Array {
  const length = (previous?.length ?? 0) + data.length;
  const next = new Uint8Array(Math.min(length, limit));
  const joined = previous ? [previous, data] : [data];
  let skip = length - next.length;
  let offset = 0;
  for (const chunk of joined) {
    const part = skip >= chunk.length ? chunk.subarray(chunk.length) : chunk.subarray(skip);
    skip = Math.max(0, skip - chunk.length);
    next.set(part, offset);
    offset += part.length;
  }
  return next;
}
