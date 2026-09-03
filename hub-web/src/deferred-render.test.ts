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
