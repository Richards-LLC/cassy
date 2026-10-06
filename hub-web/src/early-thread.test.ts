// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { ConversationHistory } from "./conversation-history";
import { gridPlaceholder, threadBeforePanes } from "./early-thread";

const at = (hh: number, mm: number) => new Date(2026, 9, 2, hh, mm).getTime();

/** A thread as a reload leaves it: only what this browser kept (cas-e7b1). */
function keptThread(): ConversationHistory {
  const history = new ConversationHistory();
  history.restorePending([{ id: "held", target: "sup", text: "Send this when it's back", state: "held", at: at(9, 0), heldAt: at(9, 0) }], at(9, 1));
  return history;
}

describe("the thread before the session attaches (cas-fc2c)", () => {
  it("shows kept messages while the conversation is still opening or reconnecting", () => {
    expect(threadBeforePanes({ placeholder: "connecting", history: keptThread() })).toBe(true);
  });

  it("waits for the session when there is nothing kept to show", () => {
    expect(threadBeforePanes({ placeholder: "connecting", history: new ConversationHistory() })).toBe(false);
    expect(threadBeforePanes({ placeholder: "connecting", history: undefined })).toBe(false);
  });

  it("leaves the thread to the supervisor's pane once it is there", () => {
    expect(threadBeforePanes({ placeholder: "none", history: keptThread() })).toBe(false);
    expect(threadBeforePanes({ placeholder: "no-panes", history: keptThread() })).toBe(false);
  });

  it("counts only what the thread would show: a dismissed failed send alone keeps the session's own opening", () => {
    const history = new ConversationHistory();
    history.restorePending([{ id: "x", target: "sup", text: "Refused", state: "error", at: at(9, 0), error: "Not sent: no" }], at(9, 1));
    history.dismissSend("x");
    expect(threadBeforePanes({ placeholder: "connecting", history })).toBe(false);
  });
});

describe("reading the grid's placeholder (cas-fc2c)", () => {
  const grid = (html: string) => { const element = document.createElement("section"); element.innerHTML = html; return element; };
  it("tells the connecting card from the attached session's empty slot and from the pane", () => {
    expect(gridPlaceholder(grid('<div class="empty"></div>'))).toBe("connecting");
    expect(gridPlaceholder(grid('<div class="conversation-mount conversation-early"></div><div class="empty conversation-opening"></div>'))).toBe("connecting");
    expect(gridPlaceholder(grid('<div class="empty empty-pane-slot"></div>'))).toBe("no-panes");
    expect(gridPlaceholder(grid('<div class="conversation-mount conversation-thread-slot"></div><div class="pane-host" hidden></div>'))).toBe("none");
    // Only the grid's own child counts: the thread's own empty state in its slot does not.
    expect(gridPlaceholder(grid('<div class="conversation-mount conversation-thread-slot"><div class="empty"></div></div><div class="pane-host" hidden></div>'))).toBe("none");
  });
});
