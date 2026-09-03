import { afterEach, describe, expect, it, vi } from "vitest";
import { anySignal, hasNativeAnySignal } from "./abort-signals";

/** Chrome 113 and every stale Android WebView reach this branch. */
function withoutNativeAny(run: () => void | Promise<void>): void | Promise<void> {
  const native = (AbortSignal as unknown as { any?: unknown }).any;
  Reflect.deleteProperty(AbortSignal as unknown as Record<string, unknown>, "any");
  try {
    return run();
  } finally {
    if (native !== undefined) Object.defineProperty(AbortSignal, "any", { value: native, configurable: true, writable: true });
  }
}

afterEach(() => vi.restoreAllMocks());

describe("anySignal", () => {
  it("uses the native implementation when the engine has one", () => {
    expect(hasNativeAnySignal()).toBe(true);
    const native = vi.spyOn(AbortSignal, "any");
    const first = new AbortController();
    anySignal([first.signal]);
    expect(native).toHaveBeenCalledOnce();
  });

  it("aborts with the first source's reason when the engine has no AbortSignal.any", () => {
    withoutNativeAny(() => {
      expect(hasNativeAnySignal()).toBe(false);
      const first = new AbortController();
      const second = new AbortController();
      const combined = anySignal([first.signal, second.signal]);
      expect(combined.aborted).toBe(false);
      second.abort(new DOMException("attaching timed out after 3s", "TimeoutError"));
      expect(combined.aborted).toBe(true);
      expect((combined.reason as DOMException).name).toBe("TimeoutError");
    });
  });

  it("starts aborted when a source is already aborted, preserving its reason", () => {
    withoutNativeAny(() => {
      const already = new AbortController();
      already.abort(new Error("event stream replaced"));
      const combined = anySignal([already.signal, new AbortController().signal]);
      expect(combined.aborted).toBe(true);
      expect((combined.reason as Error).message).toBe("event stream replaced");
    });
  });

  it("detaches its listeners once one source aborts, so a long-lived signal cannot leak them", () => {
    withoutNativeAny(() => {
      const first = new AbortController();
      const second = new AbortController();
      const removed: string[] = [];
      const spy = vi.spyOn(second.signal, "removeEventListener").mockImplementation((type) => { removed.push(String(type)); });
      anySignal([first.signal, second.signal]);
      first.abort(new Error("first"));
      expect(removed).toContain("abort");
      spy.mockRestore();
    });
  });

  it("never throws where a bare AbortSignal.any call would, which is the whole D3 failure", () => {
    withoutNativeAny(() => {
      expect(() => (AbortSignal as unknown as { any: (s: AbortSignal[]) => AbortSignal }).any([])).toThrow(TypeError);
      expect(() => anySignal([new AbortController().signal])).not.toThrow();
    });
  });
});
