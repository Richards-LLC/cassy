import { describe, expect, it } from "vitest";
import { loadPaneLayout, movePane, orderedPaneIds, promotePane, savePaneLayout, type PaneLayoutStorage } from "./pane-layout";

class MemoryStorage implements PaneLayoutStorage {
  readonly entries = new Map<string, string>();
  getItem(key: string): string | null { return this.entries.get(key) ?? null; }
  setItem(key: string, value: string): void { this.entries.set(key, value); }
}

describe("terminal pane layouts", () => {
  it("keeps the promoted supervisor first while preserving the operator's secondary order", () => {
    const storage = new MemoryStorage();
    const initial = loadPaneLayout(storage, "hub:session", ["worker-a", "supervisor", "worker-b"], "worker-a")!;
    const placed = movePane(promotePane(initial, "supervisor"), "worker-b", -1);

    savePaneLayout(storage, "hub:session", placed);

    expect(orderedPaneIds(loadPaneLayout(storage, "hub:session", ["worker-a", "supervisor", "worker-b"], "worker-a")!))
      .toEqual(["supervisor", "worker-b", "worker-a"]);
  });

  it("drops departed panes and places a newly live pane after saved placement", () => {
    const storage = new MemoryStorage();
    savePaneLayout(storage, "hub:session", { primaryPaneId: "supervisor", paneIds: ["supervisor", "worker-a"] });

    expect(loadPaneLayout(storage, "hub:session", ["supervisor", "worker-b"], "worker-b"))
      .toEqual({ primaryPaneId: "supervisor", paneIds: ["supervisor", "worker-b"] });
  });

  it("ignores an obsolete persisted schema instead of stranding a session layout", () => {
    const storage = new MemoryStorage();
    storage.setItem("cas-commander:pane-layout:hub:session", JSON.stringify({ version: 0, primaryPaneId: "worker-a", paneIds: ["worker-a"] }));

    expect(loadPaneLayout(storage, "hub:session", ["supervisor", "worker-a"], "supervisor"))
      .toEqual({ primaryPaneId: "supervisor", paneIds: ["supervisor", "worker-a"] });
  });
});
