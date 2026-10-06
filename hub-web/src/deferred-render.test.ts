import { describe, expect, it, vi } from "vitest";
import { DeferredRenderScheduler } from "./deferred-render";

function scheduler() {
  const render = vi.fn();
  const queued: (() => void)[] = [];
  const instance = new DeferredRenderScheduler({
    render,
    // The real one is a macrotask, which lands after the click that a pointerup
    // is about to produce.
    afterGesture: (run) => { queued.push(run); },
  });
  return { instance, render, drain: () => { for (const run of queued.splice(0)) run(); } };
}

describe("deferred shell rebuild", () => {
  it("does nothing when no rebuild is owed", () => {
    const { instance, render } = scheduler();

    instance.focusLeft();

    expect(render).not.toHaveBeenCalled();
  });

  it("rebuilds as soon as focus leaves the field, when no gesture is in flight", () => {
    const { instance, render } = scheduler();
    instance.defer();

    instance.focusLeft();

    expect(render).toHaveBeenCalledTimes(1);
  });

  it("rebuilds only once for one deferral", () => {
    const { instance, render } = scheduler();
    instance.defer();

    instance.focusLeft();
    instance.focusLeft();

    expect(render).toHaveBeenCalledTimes(1);
  });

  /**
   * The regression this class exists for: pointerdown moves focus off a field,
   * focusout fires, and rebuilding there replaces the button under the finger,
   * so the click never completes on the node that carried the handler.
   */
  it("does not rebuild between pointerdown and the click it will produce", () => {
    const { instance, render } = scheduler();
    instance.defer();

    instance.gestureStarted();
    instance.focusLeft();

    expect(render).not.toHaveBeenCalled();
  });

  it("rebuilds after the gesture has delivered its click", () => {
    const { instance, render, drain } = scheduler();
    instance.defer();

    instance.gestureStarted();
    instance.focusLeft();
    instance.gestureEnded();
    expect(render).not.toHaveBeenCalled();

    drain();

    expect(render).toHaveBeenCalledTimes(1);
  });

  it("keeps a rebuild owed when a gesture ends without one having been deferred", () => {
    const { instance, render, drain } = scheduler();

    instance.gestureStarted();
    instance.gestureEnded();
    drain();
    expect(render).not.toHaveBeenCalled();

    instance.defer();
    instance.focusLeft();

    expect(render).toHaveBeenCalledTimes(1);
  });

  it("survives a cancelled gesture, which produces no click at all", () => {
    const { instance, render, drain } = scheduler();
    instance.defer();

    instance.gestureStarted();
    instance.focusLeft();
    instance.gestureCancelled();
    drain();

    expect(render).toHaveBeenCalledTimes(1);
  });

  it("does not strand a rebuild when the gesture never touched a field", () => {
    const { instance, render, drain } = scheduler();

    instance.gestureStarted();
    instance.defer();
    instance.gestureEnded();
    drain();

    expect(render).toHaveBeenCalledTimes(1);
  });

  it("treats a second pointerdown before the first ends as one gesture", () => {
    const { instance, render, drain } = scheduler();
    instance.defer();

    instance.gestureStarted();
    instance.gestureStarted();
    instance.focusLeft();
    instance.gestureEnded();
    drain();

    expect(render).toHaveBeenCalledTimes(1);
  });

  it("reports whether a rebuild is still owed", () => {
    const { instance } = scheduler();
    expect(instance.pending).toBe(false);

    instance.defer();
    expect(instance.pending).toBe(true);

    instance.focusLeft();
    expect(instance.pending).toBe(false);
  });

  it("clears the debt when the page rebuilds for its own reasons", () => {
    const { instance, render } = scheduler();
    instance.defer();

    instance.settled();
    instance.focusLeft();

    expect(render).not.toHaveBeenCalled();
  });
});

/**
 * A touch tap: the browser fires pointerup when the finger lifts, then the
 * tap's mousedown (which moves focus off the field) and its click in later
 * tasks. Released on pointerup, focusout flushed the rebuild between them and
 * replaced the control under the finger (cas-207a, pairing Technical details).
 */
describe("deferred shell rebuild under a touch tap", () => {
  function touchScheduler() {
    const render = vi.fn();
    const queued: (() => void)[] = [];
    const windows: (() => void)[] = [];
    const instance = new DeferredRenderScheduler({
      render,
      afterGesture: (run) => { queued.push(run); },
      touchWindow: (run) => { windows.push(run); },
    });
    return {
      instance,
      render,
      drain: () => { for (const run of queued.splice(0)) run(); },
      expire: () => { for (const run of windows.splice(0)) run(); },
      windows,
    };
  }

  it("does not rebuild when focus leaves the field after the finger lifted, before the click", () => {
    const { instance, render, drain } = touchScheduler();
    instance.defer();

    instance.gestureStarted();
    instance.touchEnded();
    drain();
    // The tap's mousedown moves focus to the tapped control.
    instance.focusLeft();

    expect(render).not.toHaveBeenCalled();
  });

  it("rebuilds after the tap's click has been dispatched", () => {
    const { instance, render, drain } = touchScheduler();
    instance.defer();

    instance.gestureStarted();
    instance.touchEnded();
    instance.focusLeft();
    instance.clicked();
    expect(render).not.toHaveBeenCalled();
    drain();

    expect(render).toHaveBeenCalledTimes(1);
  });

  it("releases the rebuild when a lifted touch produces no click", () => {
    const { instance, render, drain, expire } = touchScheduler();
    instance.defer();

    instance.gestureStarted();
    instance.touchEnded();
    expire();
    drain();

    expect(render).toHaveBeenCalledTimes(1);
  });

  it("ignores an earlier touch's window once a later touch is waiting for its click", () => {
    const { instance, render, drain, windows } = touchScheduler();
    instance.gestureStarted();
    instance.touchEnded();
    instance.clicked();
    drain();
    instance.defer();
    instance.gestureStarted();
    instance.touchEnded();
    expect(windows).toHaveLength(2);

    // The first tap's window runs out while the second is still lifting.
    windows[0]!();
    drain();
    instance.focusLeft();
    expect(render).not.toHaveBeenCalled();

    windows[1]!();
    drain();
    expect(render).toHaveBeenCalledTimes(1);
  });

  it("treats a touch lift as the end of the gesture when no touch window is configured", () => {
    const { instance, render, drain } = scheduler();
    instance.defer();

    instance.gestureStarted();
    instance.touchEnded();
    drain();

    expect(render).toHaveBeenCalledTimes(1);
  });
});
