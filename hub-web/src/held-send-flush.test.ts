import { IDBFactory, IDBObjectStore } from "fake-indexeddb";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CommanderJournal, type CredentialFence, type DeliveryScope } from "./commander-journal";
import { HeldSendFlushes } from "./held-send-flush";
import type { PendingSend } from "./conversation-store";

afterEach(() => vi.restoreAllMocks());

const scope: DeliveryScope = { hub: "hub-a", baseUrl: "https://hub.example", device: "phone", session: "session-a" };
const fence: CredentialFence = { credentialId: "credential-a", generation: 1 };
const item: PendingSend = { id: "recovery-gap", target: "supervisor", text: "Send after recovery", state: "held", at: 1_000, heldAt: 1_000 };

describe("held-send recovery flush", () => {
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
});
