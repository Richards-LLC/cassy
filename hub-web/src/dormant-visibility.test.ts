import { describe, expect, it } from "vitest";
import {
  DORMANT_QUERY_PARAM,
  DORMANT_STORAGE_KEY,
  dormantCommandLabel,
  dormantRevealed,
  dormantRoute,
  saveDormantRevealed,
  type DormantVisibilityStorage,
} from "./dormant-visibility";

function memoryStorage(seed: Record<string, string> = {}): DormantVisibilityStorage & { readonly items: Map<string, string> } {
  const items = new Map(Object.entries(seed));
  return {
    items,
    getItem: (key) => items.get(key) ?? null,
    setItem: (key, value) => { items.set(key, value); },
    removeItem: (key) => { items.delete(key); },
  };
}

describe("dormant visibility", () => {
  it("hides dormant sessions by default and gives the route precedence", () => {
    expect(dormantRevealed("", memoryStorage())).toBe(false);
    expect(dormantRevealed("?fixture=x", undefined)).toBe(false);
    expect(dormantRevealed("?dormant=1", memoryStorage())).toBe(true);
    expect(dormantRevealed("?dormant=0", memoryStorage({ [DORMANT_STORAGE_KEY]: "1" }))).toBe(false);
  });

  it("persists only the explicit recovery choice", () => {
    const storage = memoryStorage();
    saveDormantRevealed(storage, true);
    expect(storage.items.get(DORMANT_STORAGE_KEY)).toBe("1");
    saveDormantRevealed(storage, false);
    expect(storage.items.has(DORMANT_STORAGE_KEY)).toBe(false);
    expect(() => saveDormantRevealed(undefined, true)).not.toThrow();
  });

  it("preserves other query parameters when toggling recovery", () => {
    expect(dormantRoute("?workers=1&fixture=a", true)).toBe("?workers=1&fixture=a&dormant=1");
    expect(dormantRoute("?workers=1&dormant=1", false)).toBe("?workers=1");
    expect(new URLSearchParams(dormantRoute("", true)).get(DORMANT_QUERY_PARAM)).toBe("1");
  });

  it("labels the off-by-default recovery command", () => {
    expect(dormantCommandLabel(false)).toEqual({ title: "Dormant · Hidden", hint: "Show sessions for recovery" });
    expect(dormantCommandLabel(true)).toEqual({ title: "Dormant · Shown", hint: "Hide sessions without a live supervisor" });
  });
});
