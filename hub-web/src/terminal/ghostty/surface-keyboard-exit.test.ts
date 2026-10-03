// @vitest-environment jsdom
// cas-d1fa (WCAG 2.1.2): the keyboard can always leave the terminal input.
import { describe, expect, it, vi } from "vitest";
import { GhosttyTerminalSurface, TERMINAL_ESCAPE_HINT, isTerminalFocusTab, isTerminalLeaveShortcut } from "./surface";

const key = (init: Partial<KeyboardEvent>) => ({ ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, key: "", ...init }) as KeyboardEvent;

describe("which keys leave the terminal (cas-d1fa)", () => {
  it("Ctrl+M leaves, on every platform; Cmd+M, Ctrl+Shift+M and plain m do not", () => {
    expect(isTerminalLeaveShortcut(key({ ctrlKey: true, key: "m" }))).toBe(true);
    expect(isTerminalLeaveShortcut(key({ ctrlKey: true, key: "M" }))).toBe(true);
    expect(isTerminalLeaveShortcut(key({ metaKey: true, key: "m" }))).toBe(false);
    expect(isTerminalLeaveShortcut(key({ ctrlKey: true, shiftKey: true, key: "M" }))).toBe(false);
    expect(isTerminalLeaveShortcut(key({ key: "m" }))).toBe(false);
  });

  it("plain Tab and Shift+Tab are focus keys; Ctrl, Alt or Cmd with Tab are not", () => {
    expect(isTerminalFocusTab(key({ key: "Tab" }))).toBe(true);
    expect(isTerminalFocusTab(key({ key: "Tab", shiftKey: true }))).toBe(true);
    expect(isTerminalFocusTab(key({ key: "Tab", ctrlKey: true }))).toBe(false);
    expect(isTerminalFocusTab(key({ key: "Tab", altKey: true }))).toBe(false);
    expect(isTerminalFocusTab(key({ key: "Tab", metaKey: true }))).toBe(false);
  });
});

function harness(controlMode: boolean) {
  document.body.innerHTML = '<button id="before">Before</button><div id="mount"></div><button id="after">After</button>';
  const mount = document.getElementById("mount")!;
  const input = document.createElement("textarea");
  const hint = document.createElement("p");
  hint.id = "hint";
  hint.hidden = true;
  mount.append(input, hint);
  // jsdom lays nothing out; every control counts as rendered.
  for (const element of document.querySelectorAll<HTMLElement>("button, textarea")) element.getClientRects = () => [{}] as unknown as DOMRectList;
  const surface = Object.create(GhosttyTerminalSurface.prototype) as any;
  Object.assign(surface, {
    mount, input, escapeHint: hint, controlMode, focused: true, disposed: false,
    suppressedKeyCodes: new Set<string>(), linkModifierActive: false,
    updateLinkModifier: () => undefined,
    options: { beforeKey: () => true, onData: vi.fn() },
    core: { encodeKey: () => "\t" },
  });
  return { surface, input, hint };
}

// onKeyDown is an instance field, so the key routing itself is proven in the
// browser by HUB-J12's cas-d1fa parts; here, the hint and its description.
describe("the terminal input says how to leave (cas-d1fa)", () => {
  it("shows the hint and describes the input only while the terminal keeps Tab", () => {
    const { surface, input, hint } = harness(true);
    surface.updateEscapeHint();
    expect(hint.hidden).toBe(false);
    expect(input.getAttribute("aria-describedby")).toBe("hint");
    surface.controlMode = false;
    surface.updateEscapeHint();
    expect(hint.hidden).toBe(true);
    expect(input.hasAttribute("aria-describedby")).toBe(false);
    expect(TERMINAL_ESCAPE_HINT).toBe("Tab goes to the terminal. Ctrl+M leaves it.");
  });
});
