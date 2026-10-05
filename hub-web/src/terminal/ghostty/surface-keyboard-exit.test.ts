// @vitest-environment jsdom
// cas-d1fa (WCAG 2.1.2): the keyboard can always leave the terminal input.
import { describe, expect, it } from "vitest";
import { GhosttyTerminalSurface, TERMINAL_ESCAPE_HINT_ID, isTerminalFocusTab, isTerminalLeaveShortcut } from "./surface";

const key = (init: Partial<KeyboardEvent> & { altGraph?: boolean }) => ({
  ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, key: "", code: "",
  getModifierState: (name: string) => name === "AltGraph" && init.altGraph === true,
  ...init,
}) as unknown as KeyboardEvent;

describe("which keys leave the terminal (cas-d1fa)", () => {
  it("Ctrl+Alt+M leaves, read from the physical key; Ctrl+M (Firefox's mute) does not", () => {
    expect(isTerminalLeaveShortcut(key({ ctrlKey: true, altKey: true, key: "m", code: "KeyM" }))).toBe(true);
    // Option changes the character on a Mac; the physical key still counts.
    expect(isTerminalLeaveShortcut(key({ ctrlKey: true, altKey: true, key: "µ", code: "KeyM" }))).toBe(true);
    expect(isTerminalLeaveShortcut(key({ ctrlKey: true, key: "m", code: "KeyM" }))).toBe(false);
    expect(isTerminalLeaveShortcut(key({ metaKey: true, altKey: true, key: "m", code: "KeyM" }))).toBe(false);
    expect(isTerminalLeaveShortcut(key({ ctrlKey: true, altKey: true, shiftKey: true, key: "M", code: "KeyM" }))).toBe(false);
  });

  it("AltGraph, which types characters on some layouts as Ctrl+Alt, never leaves", () => {
    expect(isTerminalLeaveShortcut(key({ ctrlKey: true, altKey: true, key: "µ", code: "KeyM", altGraph: true }))).toBe(false);
  });

  it("plain Tab and Shift+Tab are focus keys; Ctrl, Alt or Cmd with Tab are not", () => {
    expect(isTerminalFocusTab(key({ key: "Tab" }))).toBe(true);
    expect(isTerminalFocusTab(key({ key: "Tab", shiftKey: true }))).toBe(true);
    expect(isTerminalFocusTab(key({ key: "Tab", ctrlKey: true }))).toBe(false);
    expect(isTerminalFocusTab(key({ key: "Tab", altKey: true }))).toBe(false);
    expect(isTerminalFocusTab(key({ key: "Tab", metaKey: true }))).toBe(false);
  });
});

// onKeyDown is an instance field, so the key routing itself is proven in the
// browser by HUB-J12's cas-d1fa parts; here, the description wiring.
describe("the terminal input says how to leave (cas-d1fa)", () => {
  it("is described by the header's hint only while the terminal keeps Tab", () => {
    const input = document.createElement("textarea");
    const surface = Object.create(GhosttyTerminalSurface.prototype) as any;
    Object.assign(surface, { input, controlMode: true, disposed: false });
    surface.updateEscapeHint();
    expect(input.getAttribute("aria-describedby")).toBe(TERMINAL_ESCAPE_HINT_ID);
    surface.controlMode = false;
    surface.updateEscapeHint();
    expect(input.hasAttribute("aria-describedby")).toBe(false);
  });
});
