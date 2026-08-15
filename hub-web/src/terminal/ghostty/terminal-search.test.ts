import { describe, expect, it } from "vitest";
import { terminalSearchMatch } from "./surface";
import type { GhosttyRow } from "./core";

function row(text: string): GhosttyRow {
  return {
    cells: [...text].map((value) => ({
      text: value, wide: 0, foreground: { r: 0, g: 0, b: 0 }, background: { r: 0, g: 0, b: 0 }, bold: false,
      italic: false, invisible: false, strikethrough: false, overline: false,
      underline: false, selected: false,
    })),
    text,
    isWrapContinuation: false,
    wrapsToNext: false,
  };
}

describe("terminal search", () => {
  it("finds a case-insensitive visible match and returns its cell range", () => {
    expect(terminalSearchMatch([row("first"), row("Build Complete")], "complete")).toEqual({
      start: { x: 6, y: 1 },
      end: { x: 13, y: 1 },
    });
  });

  it("does not match an empty query", () => {
    expect(terminalSearchMatch([row("output")], "  ")).toBeNull();
  });
});
