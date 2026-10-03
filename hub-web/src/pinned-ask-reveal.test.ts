// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { askLineElement, revealAskLine } from "./conversation-view";

const MESSAGE = "Every lane is merged and the gate is green.\n\n- 21 tasks closed.\n- The diff is 579 files.\n- The audit docs ship with it.\n- **Ask:** open the PR to main and cut a release?";

/** A card body as the markdown renderer paints it, with layout stubbed: rows of `row` px from `top`. */
function body(rows: number, row = 20, clientHeight = 100): { element: HTMLElement; ask: HTMLElement } {
  const element = document.createElement("div");
  element.innerHTML = `<p>Every lane is merged and the gate is green.</p><ul class="markdown-list"><li>21 tasks closed.</li><li>The diff is 579 files.</li><li>The audit docs ship with it.</li><li><strong>Ask:</strong> open the PR to main and cut a release?</li></ul>`;
  document.body.append(element);
  const blocks = [...element.querySelectorAll<HTMLElement>("p, li")];
  blocks.forEach((block, index) => {
    block.getBoundingClientRect = () => ({ top: 500 + index * row - element.scrollTop, bottom: 500 + (index + 1) * row - element.scrollTop, height: row, left: 0, right: 0, width: 0, x: 0, y: 0, toJSON: () => ({}) });
  });
  element.getBoundingClientRect = () => ({ top: 500, bottom: 500 + clientHeight, height: clientHeight, left: 0, right: 0, width: 0, x: 0, y: 0, toJSON: () => ({}) });
  Object.defineProperty(element, "clientHeight", { value: clientHeight });
  Object.defineProperty(element, "scrollHeight", { value: rows * row });
  let scrollTop = 0;
  Object.defineProperty(element, "scrollTop", { get: () => scrollTop, set: (value: number) => { scrollTop = value; } });
  return { element, ask: blocks.at(-1)! };
}

describe("an opened pinned question shows its Ask line (cas-8674, journey F6)", () => {
  it("finds the block that carries the question, not the preamble", () => {
    const { element, ask } = body(5);
    expect(askLineElement(element, MESSAGE)).toBe(ask);
    expect(askLineElement(element, "")).toBeUndefined();
  });

  it("scrolls the body so the question's line ends in view", () => {
    const { element, ask } = body(5, 40, 100);
    revealAskLine(element, MESSAGE);
    // The Ask line spans 160–200 px of a 100 px body: scrolled 100, it is the last visible line.
    expect(element.scrollTop).toBe(100);
    const line = ask.getBoundingClientRect(); const box = element.getBoundingClientRect();
    expect(line.top).toBeGreaterThanOrEqual(box.top);
    expect(line.bottom).toBeLessThanOrEqual(box.bottom);
  });

  it("leaves a body that fits alone", () => {
    const { element } = body(4, 20, 100);
    revealAskLine(element, MESSAGE);
    expect(element.scrollTop).toBe(0);
  });

  it("shows the start of a question taller than the body", () => {
    const { element, ask } = body(5, 40, 30);
    revealAskLine(element, MESSAGE);
    expect(element.scrollTop).toBe(160);
    expect(ask.getBoundingClientRect().top).toBe(element.getBoundingClientRect().top);
  });
});
