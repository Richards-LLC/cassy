import { describe, expect, it } from "vitest";
import { LAUNCH_DROPPED_KEY, loadLaunchDropped, saveLaunchDropped } from "./launch-dropped";

function memoryStorage(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => { data.set(key, value); },
    removeItem: (key: string) => { data.delete(key); },
    data,
  };
}

describe("machines whose re-pair dropped starting sessions survive a reload (cas-093d F01)", () => {
  it("round-trips per machine, and an empty set is removed", () => {
    const storage = memoryStorage();
    saveLaunchDropped(storage, new Set(["atlas"]));
    expect([...loadLaunchDropped(storage)]).toEqual(["atlas"]);
    const reloaded = loadLaunchDropped(storage);
    reloaded.delete("atlas"); // allowed again, or removed from this browser
    saveLaunchDropped(storage, reloaded);
    expect(storage.data.has(LAUNCH_DROPPED_KEY)).toBe(false);
    expect(loadLaunchDropped(storage).size).toBe(0);
  });
  it("fails safe on missing, corrupted or foreign values", () => {
    expect(loadLaunchDropped(undefined).size).toBe(0);
    expect(loadLaunchDropped(memoryStorage({ [LAUNCH_DROPPED_KEY]: "{not json" })).size).toBe(0);
    expect(loadLaunchDropped(memoryStorage({ [LAUNCH_DROPPED_KEY]: JSON.stringify({ atlas: true }) })).size).toBe(0);
    expect([...loadLaunchDropped(memoryStorage({ [LAUNCH_DROPPED_KEY]: JSON.stringify(["atlas", 7, "", null, "studio"]) }))]).toEqual(["atlas", "studio"]);
    const throwing = { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("full"); }, removeItem: () => { throw new Error("denied"); } };
    expect(loadLaunchDropped(throwing).size).toBe(0);
    expect(() => saveLaunchDropped(throwing, new Set(["atlas"]))).not.toThrow();
  });
});
