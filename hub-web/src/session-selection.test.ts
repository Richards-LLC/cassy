import { describe, expect, it } from "vitest";
import {
  clearStoredSelection,
  forgetMachine,
  loadStoredSelection,
  pairedSessionToOpen,
  restorableSession,
  saveStoredSelection,
  selectionAfterPairing,
  selectSelection,
  SELECTION_HISTORY_LIMIT,
  type SelectionState,
  type SelectionStorage,
} from "./session-selection";
import type { HubSession } from "./types";

function hubSession(name: string, overrides: Partial<HubSession> = {}): HubSession {
  return { name, supervisor: "fast-kestrel-6", workers: [], liveness: "live", ...overrides };
}

function memoryStorage(seed: Record<string, string> = {}): SelectionStorage & { readonly items: Map<string, string> } {
  const items = new Map(Object.entries(seed));
  return {
    items,
    getItem: (key) => items.get(key) ?? null,
    setItem: (key, value) => { items.set(key, value); },
    removeItem: (key) => { items.delete(key); },
  };
}

describe("session selection history", () => {
  it("records the previous selection, across machines as well as sessions", () => {
    let state: SelectionState = { history: [] };
    state = selectSelection(state, { machineId: "m1", session: "alpha" });
    expect(state.history).toEqual([]);
    state = selectSelection(state, { machineId: "m2" });
    state = selectSelection(state, { machineId: "m2", session: "gamma" });
    expect(state.current).toEqual({ machineId: "m2", session: "gamma" });
    expect(state.history).toEqual([{ machineId: "m1", session: "alpha" }, { machineId: "m2" }]);
  });

  it("ignores a re-selection of the current session so history never records a no-op step", () => {
    let state: SelectionState = { history: [] };
    state = selectSelection(state, { machineId: "m1", session: "alpha" });
    state = selectSelection(state, { machineId: "m1", session: "beta" });
    const unchanged = selectSelection(state, { machineId: "m1", session: "beta" });
    expect(unchanged).toBe(state);
    expect(unchanged.history).toHaveLength(1);
  });

  it("caps the history so a long shift cannot grow it without bound", () => {
    let state: SelectionState = { history: [] };
    for (let index = 0; index <= SELECTION_HISTORY_LIMIT + 5; index += 1) {
      state = selectSelection(state, { machineId: "m1", session: `s${index}` });
    }
    // 26 selections: s25 is current, s0–s24 were pushed, and only the newest
    // SELECTION_HISTORY_LIMIT of those are retained.
    expect(state.history).toHaveLength(SELECTION_HISTORY_LIMIT);
    expect(state.history[0]?.session).toBe("s5");
    expect(state.history.at(-1)?.session).toBe("s24");
    expect(state.current?.session).toBe("s25");
  });

  it("drops a removed machine from the current selection and from the history", () => {
    let state: SelectionState = { history: [] };
    state = selectSelection(state, { machineId: "m1", session: "alpha" });
    state = selectSelection(state, { machineId: "m2", session: "beta" });
    state = selectSelection(state, { machineId: "m1", session: "gamma" });
    const pruned = forgetMachine(state, "m1");
    expect(pruned.current).toBeUndefined();
    expect(pruned.history).toEqual([{ machineId: "m2", session: "beta" }]);
  });
});

describe("where a pairing lands (cas-b452)", () => {
  it("lands a first pairing on the new machine, whatever was open", () => {
    expect(selectionAfterPairing("atlas", false, undefined)).toEqual({ machineId: "atlas" });
    expect(selectionAfterPairing("atlas", false, { machineId: "studio", session: "gabber" })).toEqual({ machineId: "atlas" });
  });

  it("returns a re-pair to the conversation it was started from, on any machine", () => {
    expect(selectionAfterPairing("atlas", true, { machineId: "atlas", session: "cas-src" })).toEqual({ machineId: "atlas", session: "cas-src" });
    expect(selectionAfterPairing("atlas", true, { machineId: "studio", session: "gabber" })).toEqual({ machineId: "studio", session: "gabber" });
  });

  it("lands a re-pair with no conversation open on the machine", () => {
    expect(selectionAfterPairing("atlas", true, undefined)).toEqual({ machineId: "atlas" });
    expect(selectionAfterPairing("atlas", true, { machineId: "studio" })).toEqual({ machineId: "atlas" });
  });
});

describe("last session restore", () => {
  it("round-trips the selection through storage", () => {
    const storage = memoryStorage();
    saveStoredSelection(storage, { machineId: "m1", session: "alpha" });
    expect(loadStoredSelection(storage)).toEqual({ machineId: "m1", session: "alpha" });
    clearStoredSelection(storage);
    expect(loadStoredSelection(storage)).toBeUndefined();
  });

  it("survives a missing store instead of blocking the app", () => {
    expect(loadStoredSelection(undefined)).toBeUndefined();
    expect(() => saveStoredSelection(undefined, { machineId: "m1" })).not.toThrow();
    expect(() => clearStoredSelection(undefined)).not.toThrow();
  });

  it("rejects unreadable, unversioned, or malformed stored selections", () => {
    expect(loadStoredSelection(memoryStorage({ "cas-commander:selection": "not json" }))).toBeUndefined();
    expect(loadStoredSelection(memoryStorage({ "cas-commander:selection": JSON.stringify({ machineId: "m1" }) }))).toBeUndefined();
    expect(loadStoredSelection(memoryStorage({ "cas-commander:selection": JSON.stringify({ version: 1, session: "alpha" }) }))).toBeUndefined();
    expect(loadStoredSelection(memoryStorage({ "cas-commander:selection": JSON.stringify({ version: 1, machineId: "m1", session: 7 }) }))).toBeUndefined();
    const throwing: SelectionStorage = {
      getItem: () => { throw new Error("denied"); },
      setItem: () => { throw new Error("denied"); },
      removeItem: () => { throw new Error("denied"); },
    };
    expect(loadStoredSelection(throwing)).toBeUndefined();
    expect(() => saveStoredSelection(throwing, { machineId: "m1" })).not.toThrow();
  });

  it("restores the stored session only once the hub lists it on that machine", () => {
    const stored = { machineId: "m1", session: "cas-src-young-raven-93" };
    expect(restorableSession(stored, "m1", [hubSession("cas-src-young-raven-93")])).toBe("cas-src-young-raven-93");
    expect(restorableSession(stored, "m1", [hubSession("gabber-studio-witty-panda-98")])).toBeUndefined();
    expect(restorableSession(stored, "m2", [hubSession("cas-src-young-raven-93")])).toBeUndefined();
    expect(restorableSession({ machineId: "m1" }, "m1", [hubSession("alpha")])).toBeUndefined();
    expect(restorableSession(undefined, "m1", [hubSession("alpha")])).toBeUndefined();
  });

  it("opens the paired machine's first live session, never a dormant or unreachable one (journey F8)", () => {
    expect(pairedSessionToOpen([hubSession("steady-wren-3")])).toBe("steady-wren-3");
    expect(pairedSessionToOpen([
      hubSession("old", { liveness: "stale_metadata" }),
      hubSession("asleep", { dormant: true }),
      hubSession("gone", { unreachable: true }),
      hubSession("first-live"),
      hubSession("second-live"),
    ])).toBe("first-live");
    expect(pairedSessionToOpen([hubSession("old", { liveness: "missing_endpoint" })])).toBeUndefined();
    expect(pairedSessionToOpen([])).toBeUndefined();
  });
});
