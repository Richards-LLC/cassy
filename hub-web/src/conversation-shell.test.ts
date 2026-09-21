// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { KEYBOARD_VIEWPORT_PROPERTY, applyKeyboardViewport, bindKeyboardViewport, keyboardViewportHeight, type VisualViewportLike } from "./conversation-shell";

class FakeVisualViewport implements VisualViewportLike {
  height: number; offsetTop = 0;
  private listeners = new Map<string, Set<() => void>>();
  constructor(height: number) { this.height = height; }
  addEventListener(type: "resize" | "scroll", listener: () => void): void { (this.listeners.get(type) ?? this.listeners.set(type, new Set()).get(type)!).add(listener); }
  removeEventListener(type: "resize" | "scroll", listener: () => void): void { this.listeners.get(type)?.delete(listener); }
  fire(type: "resize" | "scroll"): void { for (const listener of this.listeners.get(type) ?? []) listener(); }
  count(): number { return [...this.listeners.values()].reduce((n, set) => n + set.size, 0); }
}

describe("phone keyboard viewport (cas-edc9)", () => {
  it("takes the visual viewport height only when it is shorter than the layout viewport", () => {
    expect(keyboardViewportHeight(844, { height: 544 })).toBe(544);
    expect(keyboardViewportHeight(844, { height: 843.6 })).toBeUndefined();
    expect(keyboardViewportHeight(844, { height: 844 })).toBeUndefined();
    expect(keyboardViewportHeight(844, { height: 0 })).toBeUndefined();
    expect(keyboardViewportHeight(844, null)).toBeUndefined();
  });
  it("publishes the height on the root and clears it again", () => {
    applyKeyboardViewport(document, 544);
    expect(document.documentElement.style.getPropertyValue(KEYBOARD_VIEWPORT_PROPERTY)).toBe("544px");
    applyKeyboardViewport(document, undefined);
    expect(document.documentElement.style.getPropertyValue(KEYBOARD_VIEWPORT_PROPERTY)).toBe("");
  });
  it("follows visual viewport resizes, scrolls the page back to the top, and unbinds", () => {
    const visual = new FakeVisualViewport(844);
    const scrolls: [number, number][] = [];
    const win = { innerHeight: 844, visualViewport: visual, document, scrollTo: (x: number, y: number) => { scrolls.push([x, y]); } };
    const dispose = bindKeyboardViewport(win);
    expect(document.documentElement.style.getPropertyValue(KEYBOARD_VIEWPORT_PROPERTY)).toBe("");
    visual.height = 544; visual.offsetTop = 120; visual.fire("resize");
    expect(document.documentElement.style.getPropertyValue(KEYBOARD_VIEWPORT_PROPERTY)).toBe("544px");
    expect(scrolls).toEqual([[0, 0]]);
    visual.offsetTop = 0; visual.height = 844; visual.fire("resize");
    expect(document.documentElement.style.getPropertyValue(KEYBOARD_VIEWPORT_PROPERTY)).toBe("");
    expect(scrolls).toHaveLength(1);
    dispose();
    expect(visual.count()).toBe(0);
  });
  it("leaves a window without visualViewport to the meta", () => {
    const dispose = bindKeyboardViewport({ innerHeight: 844, visualViewport: null, document, scrollTo: () => { throw new Error("no scroll"); } });
    expect(document.documentElement.style.getPropertyValue(KEYBOARD_VIEWPORT_PROPERTY)).toBe("");
    dispose();
  });
});
