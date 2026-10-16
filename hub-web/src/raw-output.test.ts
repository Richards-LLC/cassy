import { describe, expect, it } from "vitest";
import {
  OutputBytesSource,
  RAW_OUTPUT_THEME,
  SCROLLBACK_PAGE_ROWS,
  ScrollbackTail,
  appendOutputTail,
  rowFromScrollback,
  rowsFromOutputBytes,
  type ScrollbackPage,
  type ScrollbackRow,
} from "./raw-output";
import { transcriptLines } from "./transcript";

const encode = (text: string) => new TextEncoder().encode(text);
const row = (screen_row: number, text = `row ${screen_row}`): ScrollbackRow => ({ screen_row, text, style_runs: [{ text, fg: [0, 0, 0], bg: [0, 0, 0], flags: 0 }] });
const page = (start: number, count: number, generation = 7, pane = "supervisor"): ScrollbackPage => ({
  pane_id: pane,
  generation,
  start_row: start,
  next_row: count > 0 ? start + count : null,
  rows: Array.from({ length: count }, (_, index) => row(start + index)),
});

describe("the daemon's rendered rows (cas-f285)", () => {
  it("keep their style runs' colour and weight, with the default foreground themed", () => {
    const rendered = rowFromScrollback({
      screen_row: 3,
      text: "ok failed",
      style_runs: [
        { text: "ok ", fg: [0, 0, 0], bg: [0, 0, 0], flags: 1 },
        { text: "failed", fg: [255, 123, 114], bg: [0, 0, 0], flags: 2 | 4 },
      ],
    });
    expect(rendered.text).toBe("ok failed");
    expect(rendered.cells[0]).toMatchObject({ text: "ok ", foreground: RAW_OUTPUT_THEME.foreground, bold: true, italic: false });
    expect(rendered.cells[1]).toMatchObject({ text: "failed", foreground: { r: 255, g: 123, b: 114 }, italic: true, underline: true, bold: false });
    const [line] = transcriptLines([rendered], RAW_OUTPUT_THEME);
    expect(line?.text).toBe("ok failed");
    expect(line?.segments.map((segment) => segment.background)).toEqual([null, null]);
  });

  it("read as plain text when a row carries no runs", () => {
    expect(rowFromScrollback({ screen_row: 0, text: "plain" }).cells.map((cell) => cell.text)).toEqual(["plain"]);
    expect(rowFromScrollback({ screen_row: 0, text: "" }).cells).toEqual([]);
  });
});

describe("an old daemon's raw bytes (cas-f285 fallback)", () => {
  it("lose their control sequences, and a carriage return redraws its line", () => {
    const bytes = encode("\u001b]0;title\u0007\u001b[1;32mready\u001b[0m\r\nprogress 10%\rprogress 100%\r\n\u001b[2Kdone\u0007\ttab\r\n");
    expect(rowsFromOutputBytes(bytes).map((line) => line.text)).toEqual(["ready", "progress 100%", "done    tab"]);
  });

  it("keep only the newest lines", () => {
    const bytes = encode(Array.from({ length: 10 }, (_, index) => `line ${index}`).join("\n"));
    expect(rowsFromOutputBytes(bytes, 3).map((line) => line.text)).toEqual(["line 7", "line 8", "line 9"]);
  });

  it("are kept as a bounded tail", () => {
    let tail = appendOutputTail(undefined, encode("abcdef"), 8);
    tail = appendOutputTail(tail, encode("ghij"), 8);
    expect(new TextDecoder().decode(tail)).toBe("cdefghij");
    expect(new TextDecoder().decode(appendOutputTail(tail, encode("0123456789"), 8))).toBe("23456789");
  });

  it("are re-read only when the bytes change", () => {
    let bytes: Uint8Array | undefined = encode("one\n");
    const source = new OutputBytesSource(() => bytes);
    const first = source.rows();
    expect(source.rows()).toBe(first);
    bytes = appendOutputTail(bytes, encode("two\n"));
    expect(source.rows().map((line) => line.text)).toEqual(["one", "two"]);
    bytes = undefined;
    expect(source.rows()).toEqual([]);
  });
});

describe("the scrollback tail Raw output reads (cas-f285)", () => {
  function tail(endRow: number) {
    const requests: Array<[number, number]> = [];
    const source = new ScrollbackTail("supervisor", 7, endRow, (start, count) => { requests.push([start, count]); return true; }, 600);
    return { source, requests };
  }

  it("reads the newest rows page by page until a short page ends them", () => {
    const { source, requests } = tail(1000);
    source.open();
    expect(requests).toEqual([[400, SCROLLBACK_PAGE_ROWS]]);
    expect(source.accept(page(400, 200))).toBe(true);
    expect(requests.at(-1)).toEqual([600, SCROLLBACK_PAGE_ROWS]);
    source.accept(page(600, 200));
    source.accept(page(800, 200));
    // The pane grew since attach: the fourth page is short and ends the tail.
    source.accept(page(1000, 12));
    expect(requests).toHaveLength(4);
    const rows = source.rows();
    expect(rows).toHaveLength(600);
    expect(rows[0]?.text).toBe("row 412");
    expect(rows.at(-1)?.text).toBe("row 1011");
  });

  it("re-reads the last page on refresh, and a refresh asked for mid-read waits its turn", () => {
    const { source, requests } = tail(150);
    source.open();
    expect(requests).toEqual([[0, SCROLLBACK_PAGE_ROWS]]);
    source.refresh();
    expect(requests).toHaveLength(1);
    source.accept(page(0, 150));
    // The owed refresh runs once the read lands.
    expect(requests.at(-1)).toEqual([0, SCROLLBACK_PAGE_ROWS]);
    const redrawn = { ...page(0, 150), rows: [...page(0, 149).rows, row(149, "redrawn")] };
    source.accept(redrawn);
    expect(source.rows().at(-1)?.text).toBe("redrawn");
  });

  it("ignores another pane's, generation's or request's page", () => {
    const { source } = tail(10);
    source.open();
    expect(source.accept(page(0, 10, 8))).toBe(false);
    expect(source.accept(page(0, 10, 7, "worker-1"))).toBe(false);
    expect(source.accept(page(5, 5))).toBe(false);
    expect(source.rows()).toEqual([]);
    expect(source.accept(page(0, 10))).toBe(true);
    expect(source.rows()).toHaveLength(10);
  });

  it("stops waiting for a page that never came back", () => {
    const { source, requests } = tail(10);
    source.open();
    source.refresh(Date.now() + 1_000);
    expect(requests).toHaveLength(1);
    source.refresh(Date.now() + 6_000);
    expect(requests).toHaveLength(2);
  });

  it("is a bounded log: nothing above it pages in", () => {
    const { source } = tail(10);
    expect(source.hasScrollbackAbove()).toBe(false);
    expect(source.theme()).toBe(RAW_OUTPUT_THEME);
  });
});
