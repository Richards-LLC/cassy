// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { HubConnectionSupervisor, HubRequestError, type HubCallbacks } from "./connection";
import { FleetOpsState, assignAction, holdAction, stopAction, type OperationRequest } from "./fleet-ops";
import type { StoredMachine } from "./types";
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
  it.each([
    [403, "permission_denied", "this pairing does not allow the action. Check its permissions in Paired machines."],
    [401, "revoked", "the pairing is no longer accepted. Pair the machine again."],
    [401, "expired", "the pairing is no longer accepted. Pair the machine again."],
  ])("explains the actual connection request's auth refusal (%s %s)", async (status, reason, detail) => {
    const connection = new HubConnectionSupervisor({ expiresAt: "2099-01-01T00:00:00Z" } as StoredMachine, {} as HubCallbacks);
    // Stub the authenticated transport, preserving request's real error class.
    Object.assign(connection, { authorizedFetch: vi.fn().mockResolvedValue({ response: new Response(null, { status }), refusal: { reason, retryable: false } }) });
    const state = new FleetOpsState(); state.select("atlas:PELICAN");
    await runFleetOperation(state, row, holdAction(agent), (request) => connection.request("POST", "/v1/sessions/PELICAN/operations", request), vi.fn());
    const text = `Could not pause swift-lark-3: ${detail}`;
    expect(state.notes.get(row)).toEqual({ text, tone: "error" });
    expect(state.announcement).toBe(text);
    expect(state.pending.size).toBe(0); expect(state.currentUndo(Date.now())).toBeUndefined();
  });

  it.each(["quiet-owl-7", null])("names the task before its assignee in failed assignments (%s)", async (worker) => {
    const state = new FleetOpsState(); state.select("atlas:PELICAN");
    const action = assignAction({ id: "cas-2001", assignee: null }, worker);
    await runFleetOperation(state, "task:cas-2001", action, async () => { throw new HubRequestError("POST /private failed (500)", 500); }, vi.fn());
    const text = `Could not ${worker ? `assign cas-2001 to ${worker}` : "unassign cas-2001"}: the machine returned an error. Try again.`;
    expect(state.notes.get("task:cas-2001")).toEqual({ text, tone: "error" });
    expect(state.announcement).toBe(text);
  });

  it.each([
    [new HubRequestError("POST /v1/sessions/private-session/operations failed (500)", 500), "the machine returned an error. Try again."],
    [new TypeError("Failed to fetch https://private-machine/v1/sessions/private-session/operations"), "the machine could not be reached. Check its connection and try again."],
    [new HubRequestError("POST /v1/sessions/private-session/operations failed (403)", 403), "this pairing does not allow the action. Check its permissions in Paired machines."],
  ])("keeps transport diagnostics out of fleet feedback (cas-a348): %s", async (error, detail) => {
    const state = new FleetOpsState(); state.select("atlas:PELICAN");
    expect(await runFleetOperation(state, row, stopAction(agent), async () => { throw error; }, vi.fn())).toBe("failed");
    const text = `Could not stop swift-lark-3: ${detail}`;
    expect(state.notes.get(row)).toEqual({ text, tone: "error" });
    expect(state.announcement).toBe(text);
    expect(state.pending.size).toBe(0); expect(state.currentUndo(Date.now())).toBeUndefined();
  });

  it("preserves actionable hub detail rather than replacing it with transport copy", async () => {
    const state = new FleetOpsState(); state.select("atlas:PELICAN");
    await runFleetOperation(state, row, holdAction(agent), async () => { throw new HubRequestError("POST /private failed (500)", 500, "audit", "Audit log unavailable. No worker was paused."); }, vi.fn());
    expect(state.announcement).toBe("Could not pause: Audit log unavailable. No worker was paused.");
  });

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
