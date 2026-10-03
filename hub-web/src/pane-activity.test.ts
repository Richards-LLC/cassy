import { describe, expect, it } from "vitest";
import { paneActivityLabel, paneShowsOutput } from "./pane-activity";

const bytes = (text: string) => [...new TextEncoder().encode(text)];

describe("the Terminal view pane header follows what the pane shows (journey F42)", () => {
  it("finds a visible glyph, and ignores escape sequences, blanks and control bytes", () => {
    expect(paneShowsOutput(bytes("The supervisor is ready.\r\n"))).toBe(true);
    expect(paneShowsOutput(bytes("\x1b[2J\x1b[H\x1b[?25l\x1b[38;5;214m   \r\n\t"))).toBe(false);
    expect(paneShowsOutput(bytes("\x1b]0;patient-pelican-9\x07\x1b]2;title\x1b\\"))).toBe(false);
    expect(paneShowsOutput(bytes("\x1b(B\x1b)0\x1b=\x1b>"))).toBe(false);
    expect(paneShowsOutput(bytes("\x1b[1m>\x1b[0m"))).toBe(true);
    expect(paneShowsOutput(bytes("\x1b[2J─"))).toBe(true);
    expect(paneShowsOutput([])).toBe(false);
  });

  it("never says No output yet above output from before this page opened", () => {
    const now = Date.parse("2026-10-03T12:00:00Z");
    expect(paneActivityLabel(undefined, true, now)).toEqual({ text: "Earlier output", title: "Output from before this page opened; nothing new since" });
    expect(paneActivityLabel(undefined, false, now)).toEqual({ text: "No output yet", title: "No output received since this page opened" });
    // Output this page saw is the time, whether or not the pane opened on earlier output.
    for (const earlier of [true, false]) {
      expect(paneActivityLabel(now - 120_000, earlier, now)).toEqual({ text: "2m", title: "2026-10-03T11:58:00.000Z" });
    }
  });
});
