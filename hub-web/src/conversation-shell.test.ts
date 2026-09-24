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

describe("cold-load list states (journey F14)", () => {
  it("shows a skeleton until storage and every machine's first catalog attempt settle", async () => {
    const { conversationListState, conversationSkeletonMarkup } = await import("./conversation-shell");
    expect(conversationListState(false, [])).toEqual({ kind: "loading" });
    expect(conversationListState(true, [{ catalogReceived: false, phase: "dialing" }, { catalogReceived: false, phase: undefined }])).toEqual({ kind: "loading" });
    // One machine answered empty, the other is still on its first attempt: not "No live supervisors" yet.
    expect(conversationListState(true, [{ catalogReceived: true, phase: "live" }, { catalogReceived: false, phase: "auth" }])).toEqual({ kind: "loading" });
    expect(conversationListState(true, [])).toEqual({ kind: "text", text: "Pair a machine to start your first conversation." });
    const empty = conversationListState(true, [{ catalogReceived: true, phase: "live" }, { catalogReceived: false, phase: "failed" }]);
    expect(empty.kind === "text" && empty.text).toMatch(/^No live supervisors listed/);
    const unreachable = conversationListState(true, [{ catalogReceived: false, phase: "backoff" }]);
    expect(unreachable.kind === "text" && unreachable.text).toMatch(/^Can't reach your paired machines yet/);
    const skeleton = document.createElement("div");
    skeleton.innerHTML = conversationSkeletonMarkup();
    expect(skeleton.querySelector('[role="status"]')?.textContent).toBe("Loading your conversations…");
    expect(skeleton.querySelectorAll(".conversation-skeleton-row")).toHaveLength(3);
  });

  it("footer says Loading… before storage and Connecting… before any machine was live", async () => {
    const { machineFooterMarkup } = await import("./paired-machines");
    const row = { id: "a", label: "Atlas", address: "atlas.test", connection: "Connecting", connected: false, lastSeen: "" };
    const text = (markup: string) => { const node = document.createElement("div"); node.innerHTML = markup; return node.querySelector("#paired-machines-toggle")?.textContent; };
    expect(text(machineFooterMarkup([], 0, "b", true))).toBe("Paired machinesLoading…");
    expect(text(machineFooterMarkup([], 0, "b"))).toBe("0 paired machinesNot paired");
    expect(text(machineFooterMarkup([row, { ...row, id: "b" }], 0, "b"))).toBe("2 paired machinesConnecting…");
    expect(text(machineFooterMarkup([row, { ...row, id: "b", everConnected: true }], 0, "b"))).toBe("2 paired machinesReconnecting");
  });
});
