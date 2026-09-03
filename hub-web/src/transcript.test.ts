import { describe, expect, it } from "vitest";
import type { GhosttyCell, GhosttyColor, GhosttyRow } from "./terminal/ghostty/core";
import { GHOSTTY_CELL_WIDE } from "./terminal/ghostty/core";
import {
  defaultTranscriptView,
  loadTranscriptView,
  saveTranscriptView,
  shouldFollowTail,
  shouldPageScrollback,
  transcriptLineKey,
  transcriptLines,
} from "./transcript";

const FOREGROUND: GhosttyColor = { r: 201, g: 209, b: 217 };
const BACKGROUND: GhosttyColor = { r: 13, g: 17, b: 23 };
const GREEN: GhosttyColor = { r: 63, g: 185, b: 80 };

interface CellOverrides {
  readonly foreground?: GhosttyColor;
  readonly background?: GhosttyColor;
  readonly bold?: boolean;
  readonly italic?: boolean;
  readonly underline?: boolean;
  readonly strikethrough?: boolean;
  readonly wide?: number;
}

function cell(text: string, overrides: CellOverrides = {}): GhosttyCell {
  return {
    text,
    wide: overrides.wide ?? GHOSTTY_CELL_WIDE.narrow,
    foreground: overrides.foreground ?? FOREGROUND,
    background: overrides.background ?? BACKGROUND,
    bold: overrides.bold ?? false,
    italic: overrides.italic ?? false,
    invisible: false,
    strikethrough: overrides.strikethrough ?? false,
    overline: false,
    underline: overrides.underline ?? false,
    selected: false,
  };
}

/** A row of `cols` cells holding `text`, padded with blanks like the real grid. */
function row(
  text: string,
  options: { cols?: number; wrapsToNext?: boolean; isWrapContinuation?: boolean; cells?: readonly GhosttyCell[] } = {},
): GhosttyRow {
  const cols = options.cols ?? 20;
  const cells = options.cells ?? [...text].map((character) => cell(character));
  const padded = [...cells];
  while (padded.length < cols) padded.push(cell(" "));
  return {
    cells: padded,
    text,
    isWrapContinuation: options.isWrapContinuation ?? false,
    wrapsToNext: options.wrapsToNext ?? false,
  };
}

const theme = { foreground: FOREGROUND, background: BACKGROUND };

describe("transcript logical lines", () => {
  it("joins a soft-wrapped grid row with its continuation into one logical line", () => {
    const lines = transcriptLines(
      [
        row("cargo check --workspace", { cols: 23, wrapsToNext: true }),
        row(" --all-targets", { cols: 23, isWrapContinuation: true }),
      ],
      theme,
    );

    expect(lines).toHaveLength(1);
    expect(lines[0]!.text).toBe("cargo check --workspace --all-targets");
  });

  it("keeps a hard newline between two unwrapped rows", () => {
    const lines = transcriptLines([row("first"), row("second")], theme);

    expect(lines.map((line) => line.text)).toEqual(["first", "second"]);
  });

  it("preserves colour, bold and underline as separate segments", () => {
    const cells = [
      cell("o", { foreground: GREEN, bold: true }),
      cell("k", { foreground: GREEN, bold: true }),
      cell(" "),
      cell("x", { underline: true }),
    ];
    const lines = transcriptLines([row("ok x", { cells, cols: 4 })], theme);

    expect(lines[0]!.segments).toEqual([
      { text: "ok", foreground: GREEN, background: null, bold: true, italic: false, underline: false, strikethrough: false },
      { text: " ", foreground: FOREGROUND, background: null, bold: false, italic: false, underline: false, strikethrough: false },
      { text: "x", foreground: FOREGROUND, background: null, bold: false, italic: false, underline: true, strikethrough: false },
    ]);
  });

  it("keeps a non-default background as a highlight span", () => {
    const cells = [cell("!", { background: GREEN })];
    const lines = transcriptLines([row("!", { cells, cols: 4 })], theme);

    expect(lines[0]!.segments[0]!.background).toEqual(GREEN);
  });

  it("drops the spacer cell that follows a wide glyph", () => {
    const cells = [
      cell("字", { wide: GHOSTTY_CELL_WIDE.wide }),
      cell("", { wide: GHOSTTY_CELL_WIDE.spacerTail }),
      cell("!"),
    ];
    const lines = transcriptLines([row("字 !", { cells, cols: 4 })], theme);

    expect(lines[0]!.text).toBe("字!");
  });

  it("trims the trailing blank rows a full-screen TUI leaves behind", () => {
    const lines = transcriptLines([row("output"), row(""), row(""), row("")], theme);

    expect(lines.map((line) => line.text)).toEqual(["output"]);
  });

  it("collapses a run of blank rows inside the screen to a single break", () => {
    const lines = transcriptLines([row("a"), row(""), row(""), row(""), row("b")], theme);

    expect(lines.map((line) => line.text)).toEqual(["a", "", "b"]);
  });
});

describe("transcript hanging indent", () => {
  it("indents a continuation under the leading whitespace of its own line", () => {
    const lines = transcriptLines([row("    at cas_cli::main")], theme);

    expect(lines[0]!.indent).toBe(4);
  });

  it("indents past an agent gutter marker so a wrap never lands at column 0", () => {
    // The defect: "› Message from @dan (ctrl+o to" wrapped and dropped
    // "expand)" to column 0, outside the › gutter.
    const lines = transcriptLines([row("› Message from @dan (ctrl+o to expand)")], theme);

    expect(lines[0]!.indent).toBe(2);
  });

  it("indents past a nested result marker and its leading whitespace", () => {
    const lines = transcriptLines([row("  ⎿  Read 42 lines")], theme);

    expect(lines[0]!.indent).toBe(5);
  });

  it("does not treat a bare word as a gutter marker", () => {
    const lines = transcriptLines([row("error: could not compile")], theme);

    expect(lines[0]!.indent).toBe(0);
  });
});

describe("transcript line identity", () => {
  it("gives an unchanged line an unchanged key so streaming reuses its node", () => {
    const [before] = transcriptLines([row("steady")], theme);
    const [after] = transcriptLines([row("steady")], theme);

    expect(transcriptLineKey(after!)).toBe(transcriptLineKey(before!));
  });

  it("changes the key when only the colour changed", () => {
    const [plain] = transcriptLines([row("ok", { cells: [cell("o"), cell("k")], cols: 2 })], theme);
    const [green] = transcriptLines(
      [row("ok", { cells: [cell("o", { foreground: GREEN }), cell("k", { foreground: GREEN })], cols: 2 })],
      theme,
    );

    expect(transcriptLineKey(green!)).not.toBe(transcriptLineKey(plain!));
  });
});

describe("transcript tail pinning", () => {
  const viewport = { scrollTop: 0, scrollHeight: 1000, clientHeight: 400 };

  it("keeps following while the reader sits at the bottom", () => {
    expect(shouldFollowTail({ ...viewport, scrollTop: 600 })).toBe(true);
  });

  it("keeps following through the sub-pixel slack a zoomed phone reports", () => {
    expect(shouldFollowTail({ ...viewport, scrollTop: 598.6 })).toBe(true);
  });

  it("stops following as soon as the reader scrolls up a line", () => {
    expect(shouldFollowTail({ ...viewport, scrollTop: 520 })).toBe(false);
  });

  it("follows a transcript shorter than its own viewport", () => {
    expect(shouldFollowTail({ scrollTop: 0, scrollHeight: 200, clientHeight: 400 })).toBe(true);
  });
});

describe("transcript scrollback paging", () => {
  it("pages the terminal back when the reader hits the top and history exists", () => {
    expect(shouldPageScrollback({ scrollTop: 0, hasScrollbackAbove: true })).toBe(true);
  });

  it("does not page when the terminal is already at the oldest row", () => {
    expect(shouldPageScrollback({ scrollTop: 0, hasScrollbackAbove: false })).toBe(false);
  });

  it("does not page while the reader is still inside the current screen", () => {
    expect(shouldPageScrollback({ scrollTop: 120, hasScrollbackAbove: true })).toBe(false);
  });
});

describe("transcript default view", () => {
  it("defaults a phone-width viewport to the transcript", () => {
    expect(defaultTranscriptView(412)).toBe("transcript");
  });

  it("defaults a desktop viewport to the terminal", () => {
    expect(defaultTranscriptView(1440)).toBe("terminal");
  });

  it("treats the compact breakpoint itself as compact", () => {
    expect(defaultTranscriptView(53 * 16)).toBe("transcript");
    expect(defaultTranscriptView(53 * 16 + 1)).toBe("terminal");
  });
});

describe("transcript view preference", () => {
  function memoryStorage() {
    const entries = new Map<string, string>();
    return {
      entries,
      getItem: (key: string) => entries.get(key) ?? null,
      setItem: (key: string, value: string) => { entries.set(key, value); },
    };
  }

  it("remembers the choice for the session that made it", () => {
    const storage = memoryStorage();
    saveTranscriptView(storage, "machine:session", "terminal");

    expect(loadTranscriptView(storage, "machine:session")).toBe("terminal");
    expect(loadTranscriptView(storage, "machine:other")).toBeUndefined();
  });

  it("ignores a corrupted or foreign stored value", () => {
    const storage = memoryStorage();
    storage.entries.set("cas-commander:transcript-view:machine:session", "{not json");

    expect(loadTranscriptView(storage, "machine:session")).toBeUndefined();
  });

  it("survives a storage that throws on write", () => {
    const throwing = {
      getItem: () => { throw new Error("denied"); },
      setItem: () => { throw new Error("denied"); },
    };

    expect(() => saveTranscriptView(throwing, "machine:session", "transcript")).not.toThrow();
    expect(loadTranscriptView(throwing, "machine:session")).toBeUndefined();
  });
});
