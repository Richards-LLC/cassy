// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { HubRequestError } from "./connection";
import { FleetOpsState, holdAction, stopAction, type OperationRequest } from "./fleet-ops";
import { runFleetOperation } from "./fleet-ops-request";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const agent = { name: "swift-lark-3", generation: "registration-2", status: "active" };
const row = `agent:${agent.name}`;
const stale = () => new HubRequestError("Stop refused", 409, "stale", "restarted", { current: { worker: agent.name, generation: "registration-3" } });
type Answer = { outcome?: Record<string, unknown> };

describe("fleet async selection/request ownership (cas-5bef0)", () => {
  for (const answer of ["success", "409"] as const) {
    it(`ignores old A ${answer} after A→B→A while a fresh same-row request is unresolved`, async () => {
      const state = new FleetOpsState(); state.select("atlas:PELICAN");
      const old = deferred<Answer>(), fresh = deferred<Answer>();
      const oldSend = vi.fn((_request: OperationRequest & { op_id: string }) => old.promise), freshSend = vi.fn((_request: OperationRequest & { op_id: string }) => fresh.promise);
      const oldRun = runFleetOperation(state, row, stopAction(agent), oldSend, vi.fn());
      state.select("studio:OTTER"); state.select("atlas:PELICAN");
      const freshAction = holdAction(agent);
      const freshRun = runFleetOperation(state, row, freshAction, freshSend, vi.fn());
      if (answer === "success") old.resolve({ outcome: {} }); else old.reject(stale());
      expect(await oldRun).toBeUndefined();
      expect(state.pending.get(row)).toBe(freshAction);
      expect(state.announcement).toBe(freshAction.progress);
      expect(state.notes.size).toBe(0); expect(state.undo).toBeUndefined();
      expect(oldSend.mock.calls[0]![0]).toMatchObject({ expected: { worker: agent.name, generation: "registration-2" } });
      expect(oldSend.mock.calls[0]![0].op_id).not.toBe(freshSend.mock.calls[0]![0].op_id);
      fresh.resolve({ outcome: {} }); expect(await freshRun).toBe("succeeded");
      expect(state.pending.size).toBe(0); expect(state.announcement).toBe(freshAction.done);
      expect(state.currentUndo(Date.now())).toBeDefined();
    });
  }

  it("retains pending ownership across same-selection DOM replacement and consumes its own409", async () => {
    const state = new FleetOpsState(); state.select("atlas:PELICAN");
    const response = deferred<Answer>(), action = stopAction(agent);
    const run = runFleetOperation(state, row, action, () => response.promise, vi.fn());
    const epoch = state.selectionEpoch;
    document.body.innerHTML = '<div id="status-view"></div>';
    // renderStatus synchronizes from state identity: a new element has no lifetime authority.
    expect(state.select("atlas:PELICAN")).toBe(false);
    expect(state.selectionEpoch).toBe(epoch); expect(state.pending.get(row)).toBe(action);
    response.reject(stale()); expect(await run).toBe("failed");
    expect(state.pending.size).toBe(0);
    expect(state.notes.get(row)).toEqual({ text: "swift-lark-3 already restarted.", tone: "stale" });
  });

  it("an earlier request cannot consume a newer slot even in the same selection with the same action object", async () => {
    const state = new FleetOpsState(); state.select("atlas:PELICAN");
    const old = deferred<Answer>(), fresh = deferred<Answer>(), action = holdAction(agent);
    const oldRun = runFleetOperation(state, row, action, () => old.promise, vi.fn());
    const freshRun = runFleetOperation(state, row, action, () => fresh.promise, vi.fn());
    old.resolve({}); expect(await oldRun).toBeUndefined();
    expect(state.pending.get(row)).toBe(action); expect(state.announcement).toBe(action.progress);
    fresh.resolve({}); expect(await freshRun).toBe("succeeded"); expect(state.pending.size).toBe(0);
  });

  it("ignores a departed selection's rejection while a different machine owns the same row", async () => {
    const state = new FleetOpsState(); state.select("atlas:PELICAN");
    const old = deferred<Answer>(), fresh = deferred<Answer>();
    const oldRun = runFleetOperation(state, row, stopAction(agent), () => old.promise, vi.fn());
    state.select("studio:OTTER");
    const action = holdAction(agent), freshRun = runFleetOperation(state, row, action, () => fresh.promise, vi.fn());
    old.reject(stale()); expect(await oldRun).toBeUndefined(); expect(state.pending.get(row)).toBe(action); expect(state.notes.size).toBe(0);
    fresh.resolve({}); expect(await freshRun).toBe("succeeded");
  });
});
