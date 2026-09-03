import { GHOSTTY_CELL_WIDE, type GhosttyCell, type GhosttyColor, type GhosttyRow } from "./terminal/ghostty/core";

/**
 * The transcript is the reflowed reading view of a pane: the emulator's own
 * logical lines rendered as wrapping text at UI size, instead of a fixed grid
 * squeezed into a phone's CSS width.
 *
 * It is built client-side from the Ghostty row snapshot, which already carries
 * the two facts a reflow needs — `isWrapContinuation` / `wrapsToNext` for soft
 * wraps, and per-cell colour and weight for ANSI styling. See hub-web/DESIGN.md
 * for the seam decision.
 */

export type TranscriptViewMode = "transcript" | "terminal";

export interface TranscriptSegment {
  readonly text: string;
  readonly foreground: GhosttyColor;
  /** Null when the cell keeps the theme background; a colour when it highlights. */
  readonly background: GhosttyColor | null;
  readonly bold: boolean;
  readonly italic: boolean;
  readonly underline: boolean;
  readonly strikethrough: boolean;
}

export interface TranscriptLine {
  readonly segments: readonly TranscriptSegment[];
  readonly text: string;
  /**
   * Columns a wrapped continuation must be pushed by so it stays inside the
   * line's own gutter rather than falling back to column 0.
   */
  readonly indent: number;
  readonly blank: boolean;
}

export interface TranscriptTheme {
  readonly foreground: GhosttyColor;
  readonly background: GhosttyColor;
}

/**
 * Gutter glyphs agent TUIs use to open a block whose continuation lines are
 * indented under it. A wrap inside such a line has to clear the gutter too.
 */
const GUTTER_MARKERS = new Set([
  "›", ">", "⎿", "•", "·", "▪", "▸", "▶", "●", "○", "⏺", "✻", "✽", "✢", "-", "*", "+", "│", "|", "┃",
]);

/** A hanging indent wide enough to swallow a phone line is worse than none. */
const MAX_INDENT = 10;

const COMPACT_BREAKPOINT_PX = 53 * 16;
const STORAGE_PREFIX = "cas-commander:transcript-view:";
const VIEW_SCHEMA_VERSION = 1;

export interface TranscriptViewStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function sameColor(left: GhosttyColor, right: GhosttyColor): boolean {
  return left.r === right.r && left.g === right.g && left.b === right.b;
}

function cellText(cell: GhosttyCell): string {
  if (cell.wide === GHOSTTY_CELL_WIDE.spacerTail) return "";
  if (cell.invisible) return " ";
  return cell.text || " ";
}

function sameStyle(left: TranscriptSegment, cell: GhosttyCell, background: GhosttyColor | null): boolean {
  return sameColor(left.foreground, cell.foreground)
    && (left.background === null ? background === null : background !== null && sameColor(left.background, background))
    && left.bold === cell.bold
    && left.italic === cell.italic
    && left.underline === cell.underline
    && left.strikethrough === cell.strikethrough;
}

/**
 * Columns to hang a wrapped continuation by: the line's own leading whitespace,
 * plus a gutter marker and the whitespace that follows it.
 */
export function transcriptIndent(text: string): number {
  const characters = [...text];
  let index = 0;
  while (index < characters.length && characters[index] === " ") index += 1;
  const marker = characters[index];
  if (marker !== undefined && GUTTER_MARKERS.has(marker) && characters[index + 1] === " ") {
    index += 1;
    while (index < characters.length && characters[index] === " ") index += 1;
  }
  return Math.min(index, MAX_INDENT);
}

function buildLine(cells: readonly GhosttyCell[], theme: TranscriptTheme): TranscriptLine {
  const segments: TranscriptSegment[] = [];
  let text = "";
  for (const cell of cells) {
    const value = cellText(cell);
    if (value === "") continue;
    text += value;
    const background = sameColor(cell.background, theme.background) ? null : cell.background;
    const last = segments.at(-1);
    if (last && sameStyle(last, cell, background)) {
      segments[segments.length - 1] = { ...last, text: last.text + value };
      continue;
    }
    segments.push({
      text: value,
      foreground: cell.foreground,
      background,
      bold: cell.bold,
      italic: cell.italic,
      underline: cell.underline,
      strikethrough: cell.strikethrough,
    });
  }
  const trimmed = text.replace(/\s+$/u, "");
  if (trimmed.length !== text.length) {
    let excess = text.length - trimmed.length;
    while (excess > 0 && segments.length > 0) {
      const last = segments[segments.length - 1]!;
      if (last.text.length <= excess) {
        excess -= last.text.length;
        segments.pop();
        continue;
      }
      segments[segments.length - 1] = { ...last, text: last.text.slice(0, last.text.length - excess) };
      excess = 0;
    }
  }
  return {
    segments,
    text: trimmed,
    indent: transcriptIndent(trimmed),
    blank: trimmed.length === 0,
  };
}

/**
 * Reflowable logical lines for a grid snapshot. Rows the emulator soft-wrapped
 * are rejoined, blank padding is trimmed, and interior blank runs collapse to a
 * single break so a redrawn TUI screen reads as a transcript rather than a form.
 */
export function transcriptLines(rows: readonly GhosttyRow[], theme: TranscriptTheme): TranscriptLine[] {
  const groups: GhosttyCell[][] = [];
  let previousWraps = false;
  for (const row of rows) {
    const continuation = row.isWrapContinuation || previousWraps;
    if (continuation && groups.length > 0) groups[groups.length - 1]!.push(...row.cells);
    else groups.push([...row.cells]);
    previousWraps = row.wrapsToNext;
  }
  const lines = groups.map((cells) => buildLine(cells, theme));
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start]!.blank) start += 1;
  while (end > start && lines[end - 1]!.blank) end -= 1;
  const collapsed: TranscriptLine[] = [];
  for (const line of lines.slice(start, end)) {
    if (line.blank && collapsed.at(-1)?.blank === true) continue;
    collapsed.push(line);
  }
  return collapsed;
}

/**
 * Identity of a rendered line. Streaming re-renders only the lines whose key
 * changed, so a chatty build does not rebuild the whole transcript each frame.
 */
export function transcriptLineKey(line: TranscriptLine): string {
  return line.segments
    .map((segment) => [
      segment.text,
      `${segment.foreground.r},${segment.foreground.g},${segment.foreground.b}`,
      segment.background ? `${segment.background.r},${segment.background.g},${segment.background.b}` : "",
      `${segment.bold ? "b" : ""}${segment.italic ? "i" : ""}${segment.underline ? "u" : ""}${segment.strikethrough ? "s" : ""}`,
    ].join(""))
    .join("");
}

export interface TranscriptViewport {
  readonly scrollTop: number;
  readonly scrollHeight: number;
  readonly clientHeight: number;
}

/**
 * Fractional CSS pixels: a zoomed phone reports a scroll position a hair short
 * of the bottom it is actually resting on.
 */
const TAIL_TOLERANCE_PX = 2;

/** Whether the transcript should keep pinning its tail as new output arrives. */
export function shouldFollowTail(viewport: TranscriptViewport): boolean {
  return viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop <= TAIL_TOLERANCE_PX;
}

/**
 * Reaching the top of the current screen pages the emulator's own scrollback
 * back, which is where transcript history lives; there is no second copy.
 */
export function shouldPageScrollback(state: { scrollTop: number; hasScrollbackAbove: boolean }): boolean {
  return state.hasScrollbackAbove && state.scrollTop <= TAIL_TOLERANCE_PX;
}

/** Transcript below the compact breakpoint, the true grid above it. */
export function defaultTranscriptView(viewportWidth: number): TranscriptViewMode {
  return viewportWidth <= COMPACT_BREAKPOINT_PX ? "transcript" : "terminal";
}

export function loadTranscriptView(storage: TranscriptViewStorage, sessionKey: string): TranscriptViewMode | undefined {
  try {
    const stored = storage.getItem(`${STORAGE_PREFIX}${sessionKey}`);
    if (!stored) return undefined;
    const parsed = JSON.parse(stored) as { version?: number; view?: string };
    if (parsed.version !== VIEW_SCHEMA_VERSION) return undefined;
    return parsed.view === "transcript" || parsed.view === "terminal" ? parsed.view : undefined;
  } catch {
    return undefined;
  }
}

export function saveTranscriptView(storage: TranscriptViewStorage, sessionKey: string, view: TranscriptViewMode): void {
  try {
    storage.setItem(`${STORAGE_PREFIX}${sessionKey}`, JSON.stringify({ version: VIEW_SCHEMA_VERSION, view }));
  } catch {
    // A private or full browser store must not prevent reading the pane.
  }
}
