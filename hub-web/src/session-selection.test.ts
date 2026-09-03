import { describe, expect, it } from "vitest";
import {
  backLabel,
  canGoBack,
  clearStoredSelection,
  forgetMachine,
  goBackSelection,
  loadStoredSelection,
  previousSelection,
  restorableSession,
  saveStoredSelection,
  selectSelection,
  sessionPickerEntries,
  sessionPickerMeta,
  workerCountLabel,
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
  it("records the previous selection so back returns to it", () => {
    let state: SelectionState = { history: [] };
    state = selectSelection(state, { machineId: "m1", session: "alpha" });
    expect(canGoBack(state)).toBe(false);
    state = selectSelection(state, { machineId: "m1", session: "beta" });
    expect(canGoBack(state)).toBe(true);
    expect(previousSelection(state)).toEqual({ machineId: "m1", session: "alpha" });
    state = goBackSelection(state);
    expect(state.current).toEqual({ machineId: "m1", session: "alpha" });
    expect(canGoBack(state)).toBe(false);
  });

  it("walks back across machines, not only sessions", () => {
    let state: SelectionState = { history: [] };
    state = selectSelection(state, { machineId: "m1", session: "alpha" });
    state = selectSelection(state, { machineId: "m2" });
    state = selectSelection(state, { machineId: "m2", session: "gamma" });
    state = goBackSelection(state);
    expect(state.current).toEqual({ machineId: "m2" });
    state = goBackSelection(state);
    expect(state.current).toEqual({ machineId: "m1", session: "alpha" });
  });

  it("ignores a re-selection of the current session so back never becomes a no-op step", () => {
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

  it("returns the same state when there is nothing to go back to", () => {
    const state: SelectionState = { current: { machineId: "m1" }, history: [] };
    expect(goBackSelection(state)).toBe(state);
  });

  it("names where back leads, falling back to the machine when no session was open", () => {
    const label = (id: string) => (id === "m1" ? "soundwave-linux" : undefined);
    expect(backLabel({ machineId: "m1", session: "cas-src-young-raven-93" }, label)).toBe("Back to cas-src-young-raven-93");
    expect(backLabel({ machineId: "m1" }, label)).toBe("Back to soundwave-linux");
    expect(backLabel({ machineId: "m9" }, label)).toBe("Back to the previous machine");
    expect(backLabel(undefined, label)).toBe("Back");
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
});

describe("session picker entries", () => {
  const machines = [{ id: "m1", label: "soundwave-linux" }, { id: "m2", label: "studio-mac" }];
  const sessions = new Map<string, HubSession[]>([
    ["m1", [
      hubSession("cas-src-young-raven-93", { supervisor: "fast-kestrel-6", workers: ["a", "b", "c", "d", "e"] }),
      hubSession("gabber-studio-witty-panda-98", { supervisor: "witty-panda-98", workers: [], liveness: "stale_metadata" }),
    ]],
    ["m2", [hubSession("studio-idle-otter-2", { supervisor: "", workers: ["z"], liveness: "missing_endpoint" })]],
  ]);

  it("lists every session the hub exposes, with the selected machine first", () => {
    const entries = sessionPickerEntries({ machines, sessions, selection: { machineId: "m2" } });
    expect(entries.map((entry) => entry.session)).toEqual([
      "studio-idle-otter-2",
      "cas-src-young-raven-93",
      "gabber-studio-witty-panda-98",
    ]);
  });

  it("carries the supervisor, worker count, and hub status for each session", () => {
    const entries = sessionPickerEntries({ machines, sessions, selection: { machineId: "m1", session: "cas-src-young-raven-93" } });
    expect(entries[0]).toMatchObject({
      machineId: "m1",
      machineLabel: "soundwave-linux",
      session: "cas-src-young-raven-93",
      role: "supervisor",
      supervisor: "fast-kestrel-6",
      workerCount: 5,
      status: "live",
      current: true,
    });
    expect(entries[1]).toMatchObject({ status: "stale metadata", current: false });
    expect(entries[2]).toMatchObject({ role: "session", supervisor: undefined, status: "missing endpoint" });
  });

  it("prefers the daemon session summary for the title and phase when one exists", () => {
    const summaries = new Map([
      ["m1:cas-src-young-raven-93", { title: "Commander session picker", phase: "editing" as const }],
    ]);
    const entries = sessionPickerEntries({ machines, sessions, selection: { machineId: "m1" }, summaries });
    expect(entries[0]).toMatchObject({ title: "Commander session picker", phase: "editing" });
    expect(entries[1].title).toBeUndefined();
    expect(entries[1].phase).toBeUndefined();
  });

  it("returns nothing when the hub has listed no sessions yet", () => {
    expect(sessionPickerEntries({ machines, sessions: new Map(), selection: { machineId: "m1" } })).toEqual([]);
  });

  it("is what the picker line says between the role and the hub status", () => {
    const [entry] = sessionPickerEntries({ machines, sessions, selection: { machineId: "m1", session: "cas-src-young-raven-93" } });
    expect(sessionPickerMeta(entry!)).toBe("supervisor fast-kestrel-6 · 5 workers · live");
  });

  it("says a worker-less session has none instead of omitting the fact", () => {
    const entries = sessionPickerEntries({ machines, sessions, selection: { machineId: "m1" } });
    const idle = entries.find((entry) => entry.session === "gabber-studio-witty-panda-98")!;
    expect(sessionPickerMeta(idle)).toBe("supervisor witty-panda-98 · no workers · stale metadata");
  });

});

describe("worker count label", () => {
  // The hub used to report an empty roster for a session running five workers,
  // so the picker hid the number rather than state a wrong one. The roster is
  // now the live registry, so zero means zero and is said out loud.
  it("counts in words a human reads, including a real zero", () => {
    expect(workerCountLabel(0)).toBe("no workers");
    expect(workerCountLabel(1)).toBe("1 worker");
    expect(workerCountLabel(5)).toBe("5 workers");
  });
});
