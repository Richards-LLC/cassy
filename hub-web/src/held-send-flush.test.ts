import { IDBFactory, IDBObjectStore } from "fake-indexeddb";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CommanderJournal, type CredentialFence, type DeliveryScope } from "./commander-journal";
import { drainHeld, HeldSendFlushes } from "./held-send-flush";
import type { PendingSend } from "./conversation-store";

afterEach(() => vi.restoreAllMocks());

const scope: DeliveryScope = { hub: "hub-a", baseUrl: "https://hub.example", device: "phone", session: "session-a" };
const fence: CredentialFence = { credentialId: "credential-a", generation: 1 };
const item: PendingSend = { id: "recovery-gap", target: "supervisor", text: "Send after recovery", state: "held", at: 1_000, heldAt: 1_000 };

describe("held-send recovery flush", () => {
  it("sends the next held message when a peer's journal sync removes the one in flight (cas-638c)", async () => {
    const inFlight = "Second tab keeps that", next = "First tab keeps this";
    const queue = [inFlight, next];
    const dispatched: string[] = [];
    await drainHeld(queue, async (entry) => {
      dispatched.push(entry);
      // The peer's claim reaches this tab's journal sync while the dispatch
      // awaits IndexedDB; the sync drops the now-sending entry from the queue.
      if (entry === inFlight) queue.splice(queue.indexOf(entry), 1);
      return () => {};
    });
    expect(dispatched).toEqual([inFlight, next]);
    expect(queue).toEqual([]);
  });

  it("keeps the last live callback while a definite no-write is settling in IndexedDB", async () => {
    const journal = new CommanderJournal(new IDBFactory(), async () => fence, () => 1_000, false);
    await journal.reconcile(scope, [], [item], fence);
    const flushes = new HeldSendFlushes();
    let live = false, writes = 0, recovery: Promise<void> | undefined;
    const flush = () => flushes.run(scope.session, async () => {
      await journal.dispatch(scope, item.id, fence, () => {
        if (!live) {
          // Recovery announces live while the active batch still awaits its
          // durable no-write settlement. There will be no later live callback.
          queueMicrotask(() => { live = true; recovery = flush(); });
          return false;
        }
        writes++;
        return true;
      });
    });
    await flush();
    await recovery;
    expect(writes).toBe(1);
    expect((await journal.read(scope)).sends[0].state).toBe("sending");
  });

  it("drains once when Live arrives before the blocked no-write settlement returns (cas-7c94)", async () => {
    const journal = new CommanderJournal(new IDBFactory(), async () => fence, () => 1_000, false);
    await journal.reconcile(scope, [], [item], fence);
    let release!: () => void, entered!: () => void;
    const settlement = new Promise<void>(resolve => { release = resolve; });
    const blocked = new Promise<void>(resolve => { entered = resolve; });
    const put = IDBObjectStore.prototype.put;
    vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (this: IDBObjectStore, value, key) {
      // Keep the real transaction and claim fence. Delay only the completion
      // callback for the first authoritative no-write -> held settlement.
      if (this.name === "sends" && value.send?.state === "held" && value.revision === 3) {
        const tx = this.transaction;
        const complete = tx.oncomplete;
        tx.oncomplete = async event => {
          entered();
          await settlement;
          complete?.call(tx, event);
        };
      }
      return put.call(this, value, key);
    });
    const flushes = new HeldSendFlushes();
    let live = false, writes = 0;
    const results: string[] = [];
    const flush = () => flushes.run(scope.session, async () => {
      results.push(await journal.dispatch(scope, item.id, fence, () => {
        if (!live) return false;
        writes++;
        return true;
      }));
    });
    const initial = flush();
    await blocked;
    expect((await journal.read(scope)).sends[0].state).toBe("held");
    live = true;
    // Machine Live and journal notifications can arrive together. They must
    // coalesce behind this batch; neither may disappear or duplicate a send.
    await Promise.all(Array.from({ length: 10 }, () => flush()));
    release();
    await initial;
    expect(results).toEqual(["waiting", "written"]);
    expect(writes).toBe(1);
    await flush();
    expect(writes).toBe(1);
  });

  it("a coalesced live callback never retries an uncertain socket write", async () => {
    const journal = new CommanderJournal(new IDBFactory(), async () => fence, () => 1_000, false);
    await journal.reconcile(scope, [], [item], fence);
    const flushes = new HeldSendFlushes();
    let writes = 0, recovery: Promise<void> | undefined;
    const flush = () => flushes.run(scope.session, async () => {
      await journal.dispatch(scope, item.id, fence, () => {
        writes++;
        queueMicrotask(() => { recovery = flush(); });
        throw new Error("socket outcome unknown");
      });
    });
    await flush();
    await recovery;
    expect(writes).toBe(1);
    expect((await journal.read(scope)).sends[0].state).toBe("sending");
  });

  it("revocation during recovery prevents the deferred write", async () => {
    const journal = new CommanderJournal(new IDBFactory(), async () => fence, () => 1_000, false);
    await journal.reconcile(scope, [], [item], fence);
    const flushes = new HeldSendFlushes();
    let writes = 0, revoked = false, recovery: Promise<void> | undefined;
    const flush = () => flushes.run(scope.session, async () => {
      await journal.dispatch(scope, item.id, fence, () => {
        if (!revoked) {
          recovery = journal.purge(scope.hub, fence).then(() => { revoked = true; return flush(); });
          return false;
        }
        writes++;
        return true;
      });
    });
    await flush();
    await recovery;
    expect(writes).toBe(0);
    expect((await journal.read(scope)).sends).toEqual([]);
  });

  // cas-5dbc (HUB-J12 queue run 38685284564): the header said Live, the
  // session's socket could not take the write yet (a definite no-write, so the
  // journal answers "waiting"), and no later live callback came. The held
  // message sat "Waiting for the connection" for good.
  it("re-attempts a batch whose write the socket could not take, with no later live callback (cas-5dbc)", async () => {
    const journal = new CommanderJournal(new IDBFactory(), async () => fence, () => 1_000, false);
    await journal.reconcile(scope, [], [item], fence);
    const flushes = new HeldSendFlushes(5);
    let writable = false, writes = 0;
    await flushes.run(scope.session, async () => {
      const result = await journal.dispatch(scope, item.id, fence, () => {
        if (!writable) return false;
        writes++;
        return true;
      });
      return result === "waiting" ? "waiting" : undefined;
    });
    expect(writes).toBe(0);
    expect((await journal.read(scope)).sends[0].state).toBe("held");
    writable = true;
    await vi.waitFor(() => expect(writes).toBe(1));
    await vi.waitFor(async () => expect((await journal.read(scope)).sends[0].state).toBe("sending"));
  });
});
