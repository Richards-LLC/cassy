import { describe, expect, it } from "vitest";
import {
  WORKERS_STORAGE_KEY,
  hiddenWorkersLabel,
  saveWorkersRevealed,
  sessionsPath,
  splitVisiblePanes,
  workersCommandLabel,
  workersRevealed,
  workersRoute,
  type WorkerVisibilityStorage,
} from "./worker-visibility";
import type { PaneInfo } from "./types";

function memoryStorage(seed: Record<string, string> = {}): WorkerVisibilityStorage & { readonly items: Map<string, string> } {
  const items = new Map(Object.entries(seed));
  return {
    items,
    getItem: (key) => items.get(key) ?? null,
    setItem: (key, value) => { items.set(key, value); },
    removeItem: (key) => { items.delete(key); },
  };
}

function pane(id: string, kind: PaneInfo["kind"]): PaneInfo {
  return { id, kind, focused: false, title: id, exited: false };
}

const panes = [pane("director", "Director"), pane("bright-otter", "Supervisor"), pane("agile-octopus", "Worker"), pane("steady-badger", "Worker")];

describe("worker visibility", () => {
  it("hides workers by default: no route flag, nothing stored", () => {
    expect(workersRevealed("", memoryStorage())).toBe(false);
    expect(workersRevealed("?fixture=x", undefined)).toBe(false);
  });

  it("reveals workers only through the explicit route flag or stored choice", () => {
    expect(workersRevealed("?workers=1", memoryStorage())).toBe(true);
    expect(workersRevealed("?workers=true", undefined)).toBe(true);
    expect(workersRevealed("?workers=0", memoryStorage({ [WORKERS_STORAGE_KEY]: "1" }))).toBe(false);
    expect(workersRevealed("", memoryStorage({ [WORKERS_STORAGE_KEY]: "1" }))).toBe(true);
    expect(workersRevealed("", memoryStorage({ [WORKERS_STORAGE_KEY]: "nonsense" }))).toBe(false);
  });

  it("stores the reveal as a value and the default as an absence", () => {
    const storage = memoryStorage();
    saveWorkersRevealed(storage, true);
    expect(storage.items.get(WORKERS_STORAGE_KEY)).toBe("1");
    saveWorkersRevealed(storage, false);
    expect(storage.items.has(WORKERS_STORAGE_KEY)).toBe(false);
    expect(() => saveWorkersRevealed(undefined, true)).not.toThrow();
  });

  it("writes the route that reproduces the current visibility", () => {
    expect(workersRoute("", true)).toBe("?workers=1");
    expect(workersRoute("?workers=1&fixture=a", false)).toBe("?fixture=a");
    expect(workersRoute("", false)).toBe("");
  });

  it("asks the catalog for workers only when revealed", () => {
    expect(sessionsPath(false)).toBe("/v1/sessions");
    expect(sessionsPath(true)).toBe("/v1/sessions?workers=1");
  });

  it("renders supervisors only by default and counts the hidden workers", () => {
    const hidden = splitVisiblePanes(panes, false);
    expect(hidden.visible.map((pane) => pane.id)).toEqual(["bright-otter"]);
    expect(hidden.hiddenWorkers.map((pane) => pane.id)).toEqual(["agile-octopus", "steady-badger"]);
    const shown = splitVisiblePanes(panes, true);
    expect(shown.visible.map((pane) => pane.id)).toEqual(["bright-otter", "agile-octopus", "steady-badger"]);
    expect(shown.hiddenWorkers).toEqual([]);
  });

  it("labels the hidden count in plain words", () => {
    expect(hiddenWorkersLabel(0)).toBe("");
    expect(hiddenWorkersLabel(1)).toBe("1 worker hidden");
    expect(hiddenWorkersLabel(4)).toBe("4 workers hidden");
    expect(workersCommandLabel(false).title).toBe("Workers · Hidden");
    expect(workersCommandLabel(true).title).toBe("Workers · Shown");
  });
});
