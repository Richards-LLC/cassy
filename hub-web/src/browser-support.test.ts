import { describe, expect, it } from "vitest";
import { REQUIRED_BROWSER_APIS, browserSupport, unsupportedBrowserNotice } from "./browser-support";

/** A probe scope where every requirement answers present unless overridden. */
function scope(overrides: Record<string, boolean> = {}): (api: string) => boolean {
  return (api) => overrides[api] ?? true;
}

describe("browser support", () => {
  it("reports nothing missing on an engine that has every required API", () => {
    const support = browserSupport(scope());
    expect(support.missing).toEqual([]);
    expect(support.supported).toBe(true);
    expect(unsupportedBrowserNotice(support)).toBeUndefined();
  });

  it("names the missing API and the minimum browsers in one line", () => {
    const support = browserSupport(scope({ "Array.prototype.toSorted": false }));
    expect(support.supported).toBe(false);
    expect(support.missing.map((requirement) => requirement.api)).toEqual(["Array.prototype.toSorted"]);
    const notice = unsupportedBrowserNotice(support);
    expect(notice).toBe(
      "This browser is missing Array.prototype.toSorted, which Cassy Commander needs. Update to Chrome 110, Edge 110, Firefox 115, or Safari 16.4 or newer.",
    );
    expect(notice?.split("\n")).toHaveLength(1);
  });

  it("raises the stated minimum to the newest floor across everything missing", () => {
    const support = browserSupport(scope({ "Array.prototype.toSorted": false, "AbortSignal.timeout": false }));
    expect(unsupportedBrowserNotice(support)).toContain("AbortSignal.timeout and Array.prototype.toSorted");
    // toSorted (110/115/16.4) is newer than AbortSignal.timeout (103/100/16).
    expect(unsupportedBrowserNotice(support)).toContain("Chrome 110, Edge 110, Firefox 115, or Safari 16.4 or newer");
  });

  it("does not gate on AbortSignal.any, because the app carries its own fallback", () => {
    expect(REQUIRED_BROWSER_APIS.map((requirement) => requirement.api)).not.toContain("AbortSignal.any");
    expect(browserSupport(scope({ "AbortSignal.any": false })).supported).toBe(true);
  });

  it("probes the real engine by default", () => {
    // Node is not a browser, so only the engine-level APIs it shares are
    // asserted here; the DOM requirements are covered by the injected probe.
    const present = new Map(browserSupport().requirements.map((requirement) => [requirement.api, requirement.present]));
    expect(present.get("AbortSignal.timeout")).toBe(true);
    expect(present.get("Array.prototype.toSorted")).toBe(true);
  });

  it("keeps every requirement backed by a real call site and a stated floor", () => {
    for (const requirement of REQUIRED_BROWSER_APIS) {
      expect(requirement.usedBy).toMatch(/\.ts/);
      expect(requirement.since.chrome).toBeGreaterThan(0);
      expect(requirement.since.safari.length).toBeGreaterThan(0);
    }
  });
});
