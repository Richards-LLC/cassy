// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { cloudBrand } from "./cloud-brand";
import { KEYBOARD_VIEWPORT_PROPERTY, conversationHeaderMarkup, applyKeyboardViewport, bindKeyboardViewport, keyboardViewportHeight, type VisualViewportLike } from "./conversation-shell";

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
    const unpaired = conversationListState(true, []);
    expect(unpaired.kind).toBe("text");
    // cas-a56c: someone sent a link by whoever runs the machine is told where it goes.
    const empty = conversationListState(true, [{ catalogReceived: true, phase: "live" }, { catalogReceived: false, phase: "failed" }]);
    expect(empty.kind).toBe("text");
    // cas-a56c: the way on is named in the controls' own words, without "recovery" jargon.
    const unreachable = conversationListState(true, [{ catalogReceived: false, phase: "backoff" }]);
    expect(unreachable.kind).toBe("text");
    // cas-97d58 F16: a browser that blocks every machine is named as the cause.
    const blocked = conversationListState(true, [{ catalogReceived: false, phase: "backoff", browserBlocked: true }]);
    expect(blocked.kind).toBe("text");
    const mixed = conversationListState(true, [{ catalogReceived: false, phase: "backoff", browserBlocked: true }, { catalogReceived: false, phase: "backoff" }]);
    expect(mixed.kind).toBe("text");
    const skeleton = document.createElement("div");
    skeleton.innerHTML = conversationSkeletonMarkup();
    expect(skeleton.querySelector('[role="status"]')?.textContent).toBeTruthy();
    expect(skeleton.querySelectorAll(".conversation-skeleton-row")).toHaveLength(3);
  });



  it("footer names never-live machines that failed as unreachable, and its dot shows the worst machine (cas-b789)", async () => {
    const { CANT_REACH_RETRYING, machineFooterMarkup } = await import("./paired-machines");
    const row = { id: "a", label: "Atlas", address: "atlas.test", connection: CANT_REACH_RETRYING, connected: false, lastSeen: "" };
    const live = { ...row, id: "b", connection: "Connected", connected: true };
    const node = (markup: string) => { const div = document.createElement("div"); div.innerHTML = markup; return div.querySelector("#paired-machines-toggle")!; };
    // Every machine failed without ever being live: the words the list uses.
    expect(node(machineFooterMarkup([row, { ...row, id: "c" }], 0, "b")).textContent).toContain("2");
    // One still on its first attempt: still connecting.
    expect(node(machineFooterMarkup([row, { ...row, id: "c", connection: "Connecting" }], 0, "b")).textContent).toContain("2");
    // Dot: green only when all are connected; warning when some are down; neutral when none.
    expect(node(machineFooterMarkup([live, { ...live, id: "c" }], 0, "b")).querySelector(".pairing-dot")?.className).toBe("pairing-dot connected");
    expect(node(machineFooterMarkup([live, row], 0, "b")).querySelector(".pairing-dot")?.className).toBe("pairing-dot partial");
    expect(node(machineFooterMarkup([row], 0, "b")).querySelector(".pairing-dot")?.className).toBe("pairing-dot");
  });
});

describe("cloudBrand", () => {
  // Contract: the sidebar brand mark is the favicon file itself (cas-8951).
  // Consumer: conversation list, thread header, pairing and paired-machines dialogs render cloudBrand().
  it("renders the brand mark from the favicon asset without shared gradient ids", () => {
    const favicon = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../public/favicon.svg"), "utf8");
    const host = document.createElement("div");
    host.innerHTML = cloudBrand() + cloudBrand();
    const marks = [...host.querySelectorAll<HTMLImageElement>(".cloud-brand > img.cloud-brand-mark")];
    expect(marks).toHaveLength(2);
    for (const mark of marks) {
      expect(mark.alt).toBe("");
      expect(decodeURIComponent(mark.getAttribute("src")!.replace("data:image/svg+xml,", ""))).toBe(favicon.trim());
    }
    // Inline SVG copies would share id="cassy-tile"; a hidden copy then breaks every visible fill.
    expect(host.querySelector("svg, [id]")).toBeNull();
    expect(host.textContent).toBeTruthy();
  });
});

describe("conversation header identity (DESIGN.md Conversation header)", () => {
  it("puts the observed connection state in a pill beside the project title (cas-bcf0)", () => {
    const model = { selected: true, loaded: true, paired: true, projectDir: "/projects/cas-src", supervisor: "lively-fox-47", host: "prowl", connectionLabel: "Live" };
    const root = document.createElement("div"); root.innerHTML = conversationHeaderMarkup(model);
    const state = root.querySelector("#conversation-connection");
    expect(state?.matches(".conversation-title-line > .status-chip")).toBe(true);
    expect(state?.textContent).toBe(model.connectionLabel);
    expect(root.querySelector(".conversation-host #conversation-connection")).toBeNull();
  });
  it("leads with the provider mark and shows model and effort, or the harness until they are reported", () => {
    const header = document.createElement("div");
    header.innerHTML = conversationHeaderMarkup({ selected: true, loaded: true, paired: true, supervisor: "calm-swan-15", projectDir: "/home/me/cassy", host: "soundwave", harness: "claude" });
    expect(header.querySelector('.conversation-avatar[data-harness="claude"] svg.provider-mark')).not.toBeNull();
    expect(header.querySelector(".conversation-host")!.textContent).toMatch(/^Claude Code/);
    header.innerHTML = conversationHeaderMarkup({ selected: true, loaded: true, paired: true, supervisor: "calm-swan-15", projectDir: "/home/me/cassy", host: "soundwave", harness: "claude", model: "claude-opus-5-5", effort: "high" });
    expect(header.querySelector(".conversation-model")!.textContent).toBeTruthy();
    header.innerHTML = conversationHeaderMarkup({ selected: true, loaded: true, paired: true, supervisor: "calm-swan-15", host: "soundwave" });
    expect(header.querySelector(".conversation-avatar")!.textContent).toBe("S");
    expect(header.querySelector(".conversation-model")).toBeNull();
  });
});
