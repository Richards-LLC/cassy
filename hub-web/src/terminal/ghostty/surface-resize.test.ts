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
    canvas: { width: 0, height: 0 },
    context: { setTransform: vi.fn() },
    metrics: { width: 10, height: 20 },
    core: { resize: vi.fn() },
    options: { onResize },
    resizeNotifyTimer: null,
    resizeNotified: false,
    canvasConfigured: false,
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
