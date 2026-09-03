import { afterEach, describe, expect, it, vi } from "vitest";
import { GhosttyTerminalSurface, shouldBlinkTerminalCursor } from "./surface";

const originalWindow = globalThis.window;

afterEach(() => {
  vi.useRealTimers();
  Object.defineProperty(globalThis, "window", { configurable: true, value: originalWindow });
});

function resizeHarness(width: number, height: number, onResize = vi.fn()) {
  const surface = Object.create(GhosttyTerminalSurface.prototype) as any;
  const mount = { clientWidth: width, clientHeight: height };
  Object.assign(surface, {
    disposed: false,
    mount,
    canvas: { width: 0, height: 0, style: {} },
    context: { setTransform: vi.fn() },
    metrics: { width: 10, height: 20 },
    core: { resize: vi.fn() },
    options: { onResize },
    resizeNotifyTimer: null,
    resizeNotified: false,
    canvasConfigured: false,
    renderScale: 1,
    authoritativeGrid: null,
    forceFullRender: false,
    scrollbarDirty: false,
    cols: 1,
    rows: 1,
    renderFrame: vi.fn(),
  });
  return { surface, mount, onResize };
}

describe("Ghostty terminal resize after a collapsed pane", () => {
  it("ignores the zero-size observation produced by a visually frozen row", () => {
    const { surface, onResize } = resizeHarness(0, 0);

    expect(surface.fit()).toBe(false);
    expect(onResize).not.toHaveBeenCalled();
  });

  it("reports one settled, real-size resize when the pane is promoted", () => {
    vi.useFakeTimers();
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: { devicePixelRatio: 1, setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout },
    });
    const { surface, mount, onResize } = resizeHarness(0, 0);

    expect(surface.fit()).toBe(false);
    mount.clientWidth = 800;
    mount.clientHeight = 600;
    expect(surface.fit()).toBe(true);
    expect(surface.fit()).toBe(true);
    expect(onResize).not.toHaveBeenCalled();

    vi.advanceTimersByTime(150);

    expect(onResize).toHaveBeenCalledTimes(1);
    expect(onResize).toHaveBeenCalledWith(surface.cols, surface.rows);
    expect(surface.cols).toBeGreaterThan(1);
    expect(surface.rows).toBeGreaterThan(1);
  });
});

describe("Ghostty terminal cursor mode", () => {
  const activeCursor = {
    focused: true,
    cursorBlinking: true,
    cursorVisible: true,
    reducedMotion: false,
  };

  it("blinks only while the real session lease is in control mode", () => {
    expect(shouldBlinkTerminalCursor({ ...activeCursor, controlMode: true })).toBe(true);
    expect(shouldBlinkTerminalCursor({ ...activeCursor, controlMode: false })).toBe(false);
  });

  it("still respects focus and reduced motion while controlling", () => {
    expect(shouldBlinkTerminalCursor({ ...activeCursor, controlMode: true, focused: false })).toBe(false);
    expect(shouldBlinkTerminalCursor({ ...activeCursor, controlMode: true, reducedMotion: true })).toBe(false);
  });
});

// cas-37f8: while the operator's dashboard owns a pane, this viewer renders the
// pane's real geometry instead of reporting (and so imposing) its own.
describe("Ghostty terminal pinned to an authoritative pane size", () => {
  function pinnedHarness(width: number, height: number) {
    const { surface, mount, onResize } = resizeHarness(width, height);
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: { devicePixelRatio: 1, setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout },
    });
    return { surface, mount, onResize };
  }

  it("renders the daemon's grid and never reports a resize back", () => {
    vi.useFakeTimers();
    // A 412px phone: 40 columns of a 10px cell, nowhere near the real 203.
    const { surface, onResize } = pinnedHarness(412, 660);

    surface.setAuthoritativeSize({ cols: 203, rows: 44 });

    expect(surface.cols).toBe(203);
    expect(surface.rows).toBe(44);
    vi.advanceTimersByTime(500);
    expect(onResize).not.toHaveBeenCalled();
  });

  it("scales the surface down so the whole authoritative grid fits the mount", () => {
    vi.useFakeTimers();
    const { surface } = pinnedHarness(412, 660);

    surface.setAuthoritativeSize({ cols: 203, rows: 44 });

    // 203 cols * 10px + 8px padding = 2038px of content in a 412px mount.
    expect(surface.renderScale).toBeCloseTo(412 / 2038, 5);
    expect(surface.canvas.style.transform).toBe(`scale(${412 / 2038})`);
    expect(surface.renderScale * 2038).toBeCloseTo(412, 5);
  });

  it("never magnifies a grid that already fits", () => {
    vi.useFakeTimers();
    const { surface } = pinnedHarness(2000, 2000);

    surface.setAuthoritativeSize({ cols: 80, rows: 24 });

    expect(surface.renderScale).toBe(1);
    expect(surface.canvas.style.transform ?? "").toBe("");
  });

  it("goes back to measuring its own mount when the dashboard releases the pane", () => {
    vi.useFakeTimers();
    const { surface, onResize } = pinnedHarness(800, 600);

    surface.setAuthoritativeSize({ cols: 203, rows: 44 });
    expect(surface.cols).toBe(203);

    surface.setAuthoritativeSize(null);
    vi.advanceTimersByTime(500);

    expect(surface.cols).not.toBe(203);
    expect(surface.renderScale).toBe(1);
    expect(onResize).toHaveBeenCalledWith(surface.cols, surface.rows);
  });
});
