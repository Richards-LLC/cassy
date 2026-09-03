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
    canvas: { width: 0, height: 0, style: {} as Record<string, string> },
    context: { setTransform: vi.fn() },
    metrics: { width: 10, height: 20 },
    core: { resize: vi.fn() },
    options: { onResize },
    resizeNotifyTimer: null,
    resizeNotified: false,
    canvasConfigured: false,
    authoritativeGrid: null,
    canvasPainting: true,
    minColumns: 0,
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

describe("Ghostty terminal minimum columns", () => {
  function stubWindow() {
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: { devicePixelRatio: 1, setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout },
    });
  }

  function phoneHarness() {
    stubWindow();
    // 395 CSS px at the smallest legible glyph is the measured phone case.
    return resizeHarness(395, 600);
  }

  it("hands the phone's own narrow grid to the PTY without a floor", () => {
    const { surface } = phoneHarness();

    expect(surface.fit()).toBe(true);
    expect(surface.cols).toBe(38);
  });

  it("holds the PTY at the floor so an 80-column TUI lays out at its designed width", () => {
    const { surface } = phoneHarness();
    surface.fit();

    surface.minColumns = 80;
    expect(surface.fit()).toBe(true);

    expect(surface.cols).toBe(80);
    expect(surface.core.resize).toHaveBeenLastCalledWith(80, expect.any(Number), 10, 20);
  });

  it("widens the canvas to the floored grid instead of squeezing it into the mount", () => {
    const { surface } = phoneHarness();
    surface.minColumns = 80;
    surface.fit();

    expect(surface.canvas.width).toBe(808);
    expect(surface.canvas.style.width).toBe("808px");
  });

  it("leaves a desktop grid and its canvas exactly as they were", () => {
    stubWindow();
    const { surface } = resizeHarness(1200, 600);
    surface.minColumns = 80;

    expect(surface.fit()).toBe(true);

    expect(surface.cols).toBe(119);
    expect(surface.canvas.width).toBe(1200);
    expect(surface.canvas.style.width).toBe("");
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
    const harness = resizeHarness(width, height);
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: { devicePixelRatio: 1, setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout },
    });
    return harness;
  }

  it("adopts the daemon's grid and never reports a resize back", () => {
    vi.useFakeTimers();
    // A 412px phone measures ~40 columns of a 10px cell; the pane is really 203.
    const { surface, onResize } = pinnedHarness(412, 660);

    surface.setAuthoritativeSize({ cols: 203, rows: 44 });

    expect(surface.cols).toBe(203);
    expect(surface.rows).toBe(44);
    vi.advanceTimersByTime(500);
    expect(onResize).not.toHaveBeenCalled();
  });

  it("outranks the phone column floor, which would still have shrunk the pane", () => {
    vi.useFakeTimers();
    const { surface, onResize } = pinnedHarness(412, 660);
    surface.setMinimumColumns(80);
    vi.advanceTimersByTime(500);
    expect(onResize).toHaveBeenCalledWith(80, surface.rows);
    onResize.mockClear();

    surface.setAuthoritativeSize({ cols: 203, rows: 44 });
    vi.advanceTimersByTime(500);

    expect(surface.cols).toBe(203);
    expect(onResize).not.toHaveBeenCalled();
  });

  it("sizes the canvas to the whole grid so none of the pane is squeezed away", () => {
    vi.useFakeTimers();
    const { surface } = pinnedHarness(412, 660);

    surface.setAuthoritativeSize({ cols: 203, rows: 44 });

    // 203 cols * 10px + 8px padding, 44 rows * 20px + 8px padding.
    expect(surface.canvas.style.width).toBe("2038px");
    expect(surface.canvas.style.height).toBe("888px");
    expect(surface.canvas.width).toBe(2038);
    expect(surface.canvas.height).toBe(888);
  });

  it("leaves a grid that already fits sized by the stylesheet", () => {
    vi.useFakeTimers();
    const { surface } = pinnedHarness(2000, 2000);

    surface.setAuthoritativeSize({ cols: 80, rows: 24 });

    expect(surface.canvas.style.width).toBe("");
    expect(surface.canvas.style.height).toBe("");
  });

  it("goes back to measuring its own mount when the dashboard releases the pane", () => {
    vi.useFakeTimers();
    const { surface, onResize } = pinnedHarness(800, 600);

    surface.setAuthoritativeSize({ cols: 203, rows: 44 });
    expect(surface.cols).toBe(203);
    onResize.mockClear();

    surface.setAuthoritativeSize(null);
    vi.advanceTimersByTime(500);

    expect(surface.cols).not.toBe(203);
    expect(onResize).toHaveBeenCalledWith(surface.cols, surface.rows);
  });
});
