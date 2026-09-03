// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { GhosttyCell, GhosttyColor, GhosttyRow } from "./terminal/ghostty/core";
import { GHOSTTY_CELL_WIDE } from "./terminal/ghostty/core";
import { TranscriptView, type TranscriptSource } from "./transcript-view";

const FOREGROUND: GhosttyColor = { r: 201, g: 209, b: 217 };
const BACKGROUND: GhosttyColor = { r: 13, g: 17, b: 23 };
const GREEN: GhosttyColor = { r: 63, g: 185, b: 80 };

function cell(text: string, overrides: Partial<GhosttyCell> = {}): GhosttyCell {
  return {
    text,
    wide: GHOSTTY_CELL_WIDE.narrow,
    foreground: FOREGROUND,
    background: BACKGROUND,
    bold: false,
    italic: false,
    invisible: false,
    strikethrough: false,
    overline: false,
    underline: false,
    selected: false,
    ...overrides,
  };
}

function row(text: string, overrides: Partial<GhosttyCell> = {}): GhosttyRow {
  return {
    cells: [...text].map((character) => cell(character, overrides)),
    text,
    isWrapContinuation: false,
    wrapsToNext: false,
  };
}

/** jsdom performs no layout, so the scroll geometry is stated explicitly. */
function stubGeometry(element: HTMLElement, geometry: { scrollHeight: number; clientHeight: number; scrollTop?: number }): void {
  Object.defineProperty(element, "scrollHeight", { configurable: true, value: geometry.scrollHeight });
  Object.defineProperty(element, "clientHeight", { configurable: true, value: geometry.clientHeight });
  Object.defineProperty(element, "scrollTop", { configurable: true, writable: true, value: geometry.scrollTop ?? 0 });
}

function source(rows: GhosttyRow[], overrides: Partial<TranscriptSource> = {}): TranscriptSource & { rowsValue: GhosttyRow[] } {
  const state: TranscriptSource & { rowsValue: GhosttyRow[] } = {
    rowsValue: rows,
    rows: (): readonly GhosttyRow[] => state.rowsValue,
    theme: () => ({ foreground: FOREGROUND, background: BACKGROUND }),
    hasScrollbackAbove: () => false,
    scrollRows: vi.fn(),
    scrollToBottom: vi.fn(),
    focus: vi.fn(),
    ...overrides,
  };
  return state as TranscriptSource & { rowsValue: GhosttyRow[] };
}

let host: TranscriptSource & { rowsValue: GhosttyRow[] };
let view: TranscriptView;

beforeEach(() => {
  host = source([row("first"), row("second")]);
  view = new TranscriptView(document, host);
  document.body.append(view.element);
  stubGeometry(view.element, { scrollHeight: 1000, clientHeight: 400 });
});

describe("transcript rendering", () => {
  it("renders one paragraph per logical line", () => {
    view.update();

    expect([...view.element.querySelectorAll(".transcript-line")].map((line) => line.textContent))
      .toEqual(["first", "second"]);
  });

  it("carries the ANSI colour onto the span, not into a canvas", () => {
    host.rowsValue = [row("ok", { foreground: GREEN, bold: true })];
    view.update();

    const span = view.element.querySelector(".transcript-line span") as HTMLElement;
    expect(span.style.color).toBe("rgb(63, 185, 80)");
    expect(span.style.fontWeight).toBe("600");
  });

  it("hangs a wrapped continuation inside the agent gutter", () => {
    host.rowsValue = [row("› Message from @dan (ctrl+o to expand)")];
    view.update();

    const line = view.element.querySelector(".transcript-line") as HTMLElement;
    expect(line.style.paddingLeft).toBe("2ch");
    expect(line.style.textIndent).toBe("-2ch");
  });

  it("reuses the node of a line that did not change while streaming", () => {
    view.update();
    const first = view.element.querySelector(".transcript-line") as HTMLElement;
    first.dataset.marked = "yes";

    host.rowsValue = [row("first"), row("second"), row("third")];
    view.update();

    expect((view.element.querySelector(".transcript-line") as HTMLElement).dataset.marked).toBe("yes");
    expect(view.element.querySelectorAll(".transcript-line")).toHaveLength(3);
  });

  it("drops the nodes of lines a redraw removed", () => {
    view.update();
    host.rowsValue = [row("only")];
    view.update();

    expect([...view.element.querySelectorAll(".transcript-line")].map((line) => line.textContent)).toEqual(["only"]);
  });

  it("repaints a line whose text changed in place", () => {
    view.update();
    host.rowsValue = [row("first"), row("second changed")];
    view.update();

    expect([...view.element.querySelectorAll(".transcript-line")].map((line) => line.textContent))
      .toEqual(["first", "second changed"]);
  });
});

describe("transcript tail", () => {
  it("pins the tail to the bottom while streaming", () => {
    view.update();

    expect(view.element.scrollTop).toBe(1000);
    expect(view.isFollowing).toBe(true);
  });

  it("offers jump to latest once the reader scrolls up", () => {
    view.update();
    view.element.scrollTop = 200;
    view.element.dispatchEvent(new Event("scroll"));

    expect(view.isFollowing).toBe(false);
    expect((view.element.querySelector(".transcript-jump") as HTMLButtonElement).hidden).toBe(false);
  });

  it("leaves a scrolled-up reader in place when new output arrives", () => {
    view.update();
    view.element.scrollTop = 200;
    view.element.dispatchEvent(new Event("scroll"));

    host.rowsValue = [row("first"), row("second"), row("third")];
    view.update();

    expect(view.element.scrollTop).toBe(200);
  });

  it("returns to the live tail when jump to latest is pressed", () => {
    view.update();
    view.element.scrollTop = 200;
    view.element.dispatchEvent(new Event("scroll"));

    (view.element.querySelector(".transcript-jump") as HTMLButtonElement).click();

    expect(host.scrollToBottom).toHaveBeenCalled();
    expect(view.isFollowing).toBe(true);
    expect(view.element.scrollTop).toBe(1000);
    expect((view.element.querySelector(".transcript-jump") as HTMLButtonElement).hidden).toBe(true);
  });

  it("resumes following when the reader scrolls back to the bottom", () => {
    view.update();
    view.element.scrollTop = 200;
    view.element.dispatchEvent(new Event("scroll"));
    view.element.scrollTop = 600;
    view.element.dispatchEvent(new Event("scroll"));

    expect(view.isFollowing).toBe(true);
    expect((view.element.querySelector(".transcript-jump") as HTMLButtonElement).hidden).toBe(true);
  });
});

describe("transcript keyboard", () => {
  it("opens the pane keyboard when the transcript is tapped", () => {
    view.update();
    (view.element.querySelector(".transcript-line") as HTMLElement).click();

    expect(host.focus).toHaveBeenCalled();
  });

  it("does not steal focus when the jump affordance is the target", () => {
    view.update();
    view.element.scrollTop = 200;
    view.element.dispatchEvent(new Event("scroll"));
    (view.element.querySelector(".transcript-jump") as HTMLButtonElement).click();

    expect(host.focus).not.toHaveBeenCalled();
  });
});

describe("transcript scrollback", () => {
  it("pages the terminal back when the reader reaches the top", () => {
    host.hasScrollbackAbove = () => true;
    view.update();
    view.element.scrollTop = 0;
    view.element.dispatchEvent(new Event("scroll"));

    expect(host.scrollRows).toHaveBeenCalledWith(-5);
  });

  it("does not page when the session has no scrollback above", () => {
    view.update();
    view.element.scrollTop = 0;
    view.element.dispatchEvent(new Event("scroll"));

    expect(host.scrollRows).not.toHaveBeenCalled();
  });
});
