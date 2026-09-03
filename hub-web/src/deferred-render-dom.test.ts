// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DeferredRenderScheduler } from "./deferred-render";

/**
 * The cas-c142 regression, reproduced against real DOM events rather than
 * described: a rebuild flushed on focusout replaces the button between
 * pointerdown and click, so the click is delivered to a detached node and the
 * handler never runs. The pairing dialog's Cancel was the instance that
 * surfaced it; the mechanism belongs to any control clicked while a field has
 * focus.
 */
function shell(root: HTMLElement, onAction: () => void): void {
  root.innerHTML = `<input id="field"><button id="action">Cancel</button>`;
  root.querySelector<HTMLButtonElement>("#action")!.onclick = onAction;
}

interface PressResult {
  /** Whether the node that received pointerdown was still in the document
   *  when the browser came to dispatch its click. A detached one means the
   *  operator pressed a button that no longer exists. */
  readonly pressedStillConnected: boolean;
  readonly order: readonly string[];
}

interface Harness {
  readonly root: HTMLElement;
  readonly action: () => number;
  readonly rebuilds: () => number;
  press(): PressResult;
}

/** Wires a scheduler the way main.ts does and drives a real press gesture. */
function harness(options: { gestureAware: boolean }): Harness {
  const root = document.createElement("div");
  document.body.replaceChildren(root);
  let actionCalls = 0;
  let rebuilds = 0;
  const rebuild = (): void => {
    rebuilds += 1;
    shell(root, () => { actionCalls += 1; });
  };
  shell(root, () => { actionCalls += 1; });

  // main.ts schedules this with setTimeout(0), which the browser runs after it
  // has dispatched the click; the harness drains it at the same point.
  const afterGesture: (() => void)[] = [];
  const scheduler = new DeferredRenderScheduler({
    render: rebuild,
    afterGesture: (run) => { afterGesture.push(run); },
  });
  scheduler.defer();

  if (options.gestureAware) {
    root.addEventListener("pointerdown", () => scheduler.gestureStarted(), true);
    root.addEventListener("pointerup", () => scheduler.gestureEnded(), true);
  }
  root.addEventListener("focusout", () => scheduler.focusLeft());

  return {
    root,
    action: () => actionCalls,
    rebuilds: () => rebuilds,
    press(): PressResult {
      const order: string[] = [];
      root.addEventListener("pointerup", () => order.push("pointerup"), true);
      root.addEventListener("click", () => order.push("click"), true);
      const field = root.querySelector<HTMLInputElement>("#field")!;
      const pressed = root.querySelector<HTMLButtonElement>("#action")!;
      field.focus();
      // The browser order for a tap on a button while a field has focus.
      pressed.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      field.blur();
      pressed.dispatchEvent(new Event("pointerup", { bubbles: true }));
      const pressedStillConnected = pressed.isConnected;
      // The click goes to whatever is under the pointer now.
      root.querySelector<HTMLButtonElement>("#action")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      for (const run of afterGesture.splice(0)) run();
      return { pressedStillConnected, order };
    },
  };
}

beforeEach(() => {
  vi.restoreAllMocks();
});

describe("a click that lands during a deferred rebuild", () => {
  it("destroys the pressed button mid-gesture when the rebuild flushes on focus alone — the cas-c142 defect", () => {
    const flat = harness({ gestureAware: false });

    const result = flat.press();

    expect(flat.rebuilds()).toBe(1);
    // In Chromium this is exactly what made the tap do nothing: measured on
    // the pairing dialog, the click produced no close() at all.
    expect(result.pressedStillConnected).toBe(false);
  });

  it("keeps the pressed button alive through the gesture once the rebuild waits", () => {
    const guarded = harness({ gestureAware: true });

    const result = guarded.press();

    expect(result.pressedStillConnected).toBe(true);
    expect(guarded.action()).toBe(1);
    expect(guarded.rebuilds()).toBe(1);
  });

  it("rebuilds after the click, not instead of it", () => {
    const guarded = harness({ gestureAware: true });

    const result = guarded.press();

    expect(result.order).toEqual(["pointerup", "click"]);
    expect(guarded.rebuilds()).toBe(1);
  });

  it("leaves the field alone while focus merely moves between inputs", () => {
    const guarded = harness({ gestureAware: true });

    guarded.root.querySelector<HTMLInputElement>("#field")!.focus();

    expect(guarded.rebuilds()).toBe(0);
  });
});
