import { IDBFactory } from "fake-indexeddb";
import { describe, expect, it } from "vitest";
import { CommanderJournal, deliveryScope, DELIVERY_DB, type CredentialFence, type DeliveryScope } from "./commander-journal";
import type { PendingSend } from "./conversation-store";
import { ConversationHistory } from "./conversation-history";

const scope: DeliveryScope = { hub: "hub-a", baseUrl: "https://hub.example", device: "phone", session: "session-a" };
const fence: CredentialFence = { credentialId: "credential-a", generation: 1 };
const send = (id: string): PendingSend => ({ id, target: "supervisor", text: `message ${id}`, state: "held", at: 1_000, heldAt: 1_000 });
const reply = { notification_id: 42, reply_to: null, message: "Reply", summary: "", device_id: "phone" };
function journals(now = () => 1_000) {
  const db = new IDBFactory();
  const make = () => new CommanderJournal(db, async () => fence, now, false);
  return { a: make(), b: make(), make, db };
}

describe("atomic Commander journal", () => {
  it("requeues an existing history row after a claim makes no socket write (cas-9dc6)", async () => {
    const db = new IDBFactory(), history = new ConversationHistory();
    const peer = new CommanderJournal(db, async () => fence, () => 1_000, false);
    let inspectClaim = false, observedClaim = false;
    const writer = new CommanderJournal(db, async () => {
      if (inspectClaim) {
        const snapshot = await peer.read(scope);
        if (snapshot.sends[0]?.state === "sending") {
          observedClaim = true;
          // The real onChange path sees the durable claim before the socket
          // readiness check returns false, and removes its in-memory queue.
          expect(history.synchronizePending(snapshot.sends, 1_010)).toEqual([]);
          expect(history.pendingSends()[0].state).toBe("unconfirmed");
        }
      }
      return fence;
    }, () => 1_000, false);
    const item = send("readiness-gap");
    await writer.reconcile(scope, [], [item], fence);
    history.restorePending((await peer.read(scope)).sends, 1_000);
    inspectClaim = true;
    expect(await writer.dispatch(scope, item.id, fence, () => false)).toBe("waiting");
    inspectClaim = false;
    expect(observedClaim).toBe(true);
    const reset = await peer.read(scope);
    expect(history.synchronizePending(reset.sends, 1_020)).toEqual([item]);
    expect(history.events).toHaveLength(1);
    expect(history.pendingSends()[0]).toMatchObject({ id: item.id, state: "held" });
    expect(history.pendingSends()[0].sentAt).toBeUndefined();
    expect(history.synchronizePending(reset.sends, 1_030), "already queued rows are not queued twice").toEqual([]);
    let writes = 0;
    expect(await peer.dispatch(scope, item.id, fence, () => { writes++; return true; })).toBe("written");
    await peer.acknowledge(scope, { client_ref: item.id, notification_id: 99, target: item.target, stamped: true }, fence);
    const confirmed = await writer.read(scope);
    expect(history.synchronizePending(confirmed.sends, 1_040, confirmed.receipts)).toEqual([]);
    expect(history.synchronizePending(reset.sends, 1_050), "a stale held snapshot cannot requeue a delivered row").toEqual([]);
    expect(history.events).toHaveLength(1);
    expect(writes).toBe(1);
    const event = history.events[0];
    expect(event.kind === "send" && history.showsDelivered(event.value)).toBe(true);
  });
  it("an authoritative no-delivery refusal makes Cancel durable and strips the private payload", async () => {
    const { a, b, make, db } = journals();
    const item = send("refused");
    await a.reconcile(scope, [], [item], fence);
    expect(await a.dispatch(scope, item.id, fence, () => true)).toBe("written");
    expect(await b.refuse(scope, item.id, fence, "upstream unavailable", true)).toBe("stale");
    expect(await a.refuse(scope, item.id, fence, "upstream unavailable", true)).toBe("held");
    expect((await b.read(scope)).sends[0].state).toBe("held");
    expect(await b.cancel(scope, item.id)).toBe(true);
    const request = db.open(DELIVERY_DB, 1);
    const records = await new Promise<unknown[]>(resolve => { request.onsuccess = () => {
      const database = request.result, tx = database.transaction("sends", "readonly"), read = tx.objectStore("sends").getAll();
      tx.oncomplete = () => { database.close(); resolve(read.result); };
    }; });
    expect(JSON.stringify(records)).not.toContain(item.text);
    expect(records).toHaveLength(1); // Retain the reference fence, never the text.
    let writes = 0;
    expect(await make().dispatch(scope, item.id, fence, () => { writes++; return true; })).toBe("unconfirmed");
    expect(writes).toBe(0);
  });
  it("an old refusal cannot re-hold a newer same-tab Retry; the newer attempt can still be refused", async () => {
    const { a, b } = journals();
    const item = send("retried");
    await a.reconcile(scope, [], [item], fence);
    let writes = 0;
    await a.dispatch(scope, item.id, fence, () => { writes++; return true; });
    expect(await a.retry(scope, item.id, fence, item)).toBe("kept");
    await a.dispatch(scope, item.id, fence, () => { writes++; return true; });
    expect(await a.refuse(scope, item.id, fence, "late old refusal", true)).toBe("stale");
    expect(await b.dispatch(scope, item.id, fence, () => { writes++; return true; })).toBe("unconfirmed");
    expect(writes).toBe(2);
    expect(await a.refuse(scope, item.id, fence, "new refusal", true)).toBe("held");
    expect(await b.dispatch(scope, item.id, fence, () => { writes++; return true; })).toBe("written");
    expect(writes).toBe(3);
  });
  it("receipts and cancellation beat late refusals and freshly observed stale private captions", async () => {
    const { a, b } = journals();
    const confirmed = send("confirmed"), cancelled = send("cancelled");
    await a.reconcile(scope, [], [confirmed, cancelled], fence);
    await a.dispatch(scope, confirmed.id, fence, () => true);
    await a.acknowledge(scope, { client_ref: confirmed.id, notification_id: 99, target: confirmed.target, stamped: true }, fence);
    expect(await a.refuse(scope, confirmed.id, fence, "late", true)).toBe("stale");
    await a.dispatch(scope, cancelled.id, fence, () => true);
    expect(await a.refuse(scope, cancelled.id, fence, "unavailable", true)).toBe("held");
    expect(await a.cancel(scope, cancelled.id)).toBe(true);
    // Even after reading the terminal revision, stale non-held captions must
    // not restore the private payload or make a reference dispatchable.
    await b.read(scope);
    await b.reconcile(scope, [cancelled], [{ ...cancelled, state: "unconfirmed" }], fence);
    expect((await b.read(scope)).sends).toEqual([]);
    expect((await b.read(scope)).receipts[0].notification_id).toBe(99);
  });
  it("a nonretryable refusal stays Not sent, and consumes its attempt before a later Retry", async () => {
    const { a } = journals();
    const item = send("refused");
    await a.reconcile(scope, [], [item], fence);
    await a.dispatch(scope, item.id, fence, () => true);
    expect(await a.refuse(scope, item.id, fence, "control required", false)).toBe("refused");
    expect((await a.read(scope)).sends[0]).toMatchObject({ state: "error", error: "control required" });
    expect(await a.retry(scope, item.id, fence, item)).toBe("kept");
    await a.dispatch(scope, item.id, fence, () => true);
    expect(await a.refuse(scope, item.id, fence, "unavailable", true)).toBe("held");
  });

  it("one confirmed ref settles both tab histories without retaining a private send payload (cas-9dc6)", async () => {
    const { a, b, db } = journals();
    const first = new ConversationHistory(), second = new ConversationHistory();
    const item = send("shared");
    await a.reconcile(scope, [], [item], fence);
    first.restorePending((await a.read(scope)).sends, 1_000);
    second.restorePending((await b.read(scope)).sends, 1_000);
    await a.dispatch(scope, item.id, fence, () => true);
    const receipt = { client_ref: item.id, notification_id: 99, target: item.target, stamped: true };
    expect(await a.acknowledge(scope, receipt, fence)).toBe(true);
    for (const [journal, history] of [[a, first], [b, second]] as const) {
      const snapshot = await journal.read(scope);
      expect(snapshot.sends).toEqual([]);
      history.synchronizePending(snapshot.sends, 1_000, snapshot.receipts);
      const event = history.events[0];
      expect(event?.kind === "send" && history.showsDelivered(event.value)).toBe(true);
      expect(event?.kind === "send" && history.isFailedSend(event.value)).toBe(false);
    }
    const request = db.open(DELIVERY_DB, 1);
    const records = await new Promise<unknown[]>(resolve => { request.onsuccess = () => {
      const database = request.result, tx = database.transaction("sends", "readonly"), read = tx.objectStore("sends").getAll();
      tx.oncomplete = () => { database.close(); resolve(read.result); };
    }; });
    expect(JSON.stringify(records)).not.toContain(item.text);
  });
  it("a stale Retry or journal snapshot cannot create another write after confirmation (cas-9dc6)", async () => {
    const { a, b } = journals();
    const item = send("shared");
    await a.reconcile(scope, [], [item], fence);
    await b.read(scope);
    let writes = 0;
    await a.dispatch(scope, item.id, fence, () => { writes++; return true; });
    await a.acknowledge(scope, { client_ref: item.id, notification_id: 99, target: item.target, stamped: true }, fence);
    expect(await b.retry(scope, item.id, fence, item)).toBe("delivered");
    await b.reconcile(scope, [item], [item], fence);
    await b.dispatch(scope, item.id, fence, () => { writes++; return true; });
    expect(writes).toBe(1);
    expect((await b.read(scope)).receipts[0].notification_id).toBe(99);
  });
  it("explicit uncertain Retry retains the wire ref, while concurrent stale retries cannot re-claim it (cas-9dc6)", async () => {
    const { a, b } = journals();
    const item = send("shared");
    await a.reconcile(scope, [], [item], fence);
    await a.dispatch(scope, item.id, fence, () => true);
    await b.read(scope);
    const retries = await Promise.all([a.retry(scope, item.id, fence, item), b.retry(scope, item.id, fence, item)]);
    expect(retries.filter(value => value === "kept")).toHaveLength(1);
    expect((await a.read(scope)).sends[0].id).toBe(item.id);
    expect(await b.retry(scope, item.id, fence, { ...item, text: "different content" })).toBe("not-saved");
  });
  it("a peer's unconfirmed caption cannot undo the writer's explicit Retry (cas-9dc6 F01)", async () => {
    const { a, b } = journals();
    const item = send("shared"), peerHistory = new ConversationHistory();
    await a.reconcile(scope, [], [item], fence);
    await a.dispatch(scope, item.id, fence, () => true);
    peerHistory.restorePending((await b.read(scope)).sends, 1_000);
    expect(await a.retry(scope, item.id, fence, item)).toBe("kept");
    // The broadcast refreshes this tab's observed revision, but its existing
    // unconfirmed bubble is still present when caption persistence runs.
    const snapshot = await b.read(scope);
    peerHistory.synchronizePending(snapshot.sends, 1_000, snapshot.receipts);
    await b.reconcile(scope, snapshot.sends, peerHistory.pendingSends(), fence);
    let writes = 0;
    expect(await a.dispatch(scope, item.id, fence, () => { writes++; return true; })).toBe("written");
    expect(writes).toBe(1);
    expect((await b.read(scope)).sends[0].id).toBe(item.id);
  });
  it("a retry stays dispatchable by a peer if the writer closes before dispatch (cas-9dc6 F01)", async () => {
    const { a, b } = journals();
    const item = send("shared");
    await a.reconcile(scope, [], [item], fence);
    await a.dispatch(scope, item.id, fence, () => true);
    expect(await a.retry(scope, item.id, fence, item)).toBe("kept");
    let writes = 0;
    expect(await b.dispatch(scope, item.id, fence, () => { writes++; return true; })).toBe("written");
    expect(writes).toBe(1);
  });
  it("receipt persistence honours credential fences and revocation (cas-9dc6)", async () => {
    const { a } = journals();
    const item = send("shared"), receipt = { client_ref: item.id, notification_id: 99, target: item.target, stamped: true };
    await a.reconcile(scope, [], [item], fence);
    await a.dispatch(scope, item.id, fence, () => true);
    expect(await a.acknowledge(scope, receipt, { ...fence, generation: 2 })).toBe(false);
    await a.purge(scope.hub, fence);
    expect(await a.acknowledge(scope, receipt, fence)).toBe(false);
    expect((await a.read(scope)).receipts).toEqual([]);
  });
  it("two tabs insert different items without overwriting the other snapshot", async () => {
    const { a, b } = journals();
    await Promise.all([a.reconcile(scope, [], [send("a")], fence), b.reconcile(scope, [], [send("b")], fence)]);
    expect((await a.read(scope)).sends.map((row) => row.id).sort()).toEqual(["a", "b"]);
  });
  it("100 simultaneous cross-tab claims authorize exactly one write", async () => {
    const { a, b } = journals();
    await a.reconcile(scope, [], [send("a")], fence);
    let writes = 0;
    const outcomes = await Promise.all(Array.from({ length: 100 }, (_, i) => (i % 2 ? a : b).dispatch(scope, "a", fence, () => { writes++; return true; })));
    expect(writes).toBe(1);
    expect(outcomes.filter((outcome) => outcome === "written")).toHaveLength(1);
    expect((await a.read(scope)).sends[0].state).toBe("sending");
  });
  it("a stale held caption from the claiming tab cannot reopen a completed wire claim (cas-9dc6 recovery)", async () => {
    const { a, b } = journals();
    const history = new ConversationHistory();
    const item = send("shared");
    await a.reconcile(scope, [], [item], fence);
    history.restorePending((await a.read(scope)).sends, 1_000);
    // main.ts captures a held History snapshot before dispatch; a broadcast
    // can refresh the persisted/observed journal revision before it commits.
    const staleCaption = history.pendingSends();
    let writes = 0;
    await a.dispatch(scope, item.id, fence, () => { writes++; return true; });
    const claimed = await a.read(scope);
    await a.reconcile(scope, claimed.sends, staleCaption, fence);
    await b.dispatch(scope, item.id, fence, () => { writes++; return true; });
    expect(writes).toBe(1);
    expect((await b.read(scope)).sends[0].state).toBe("sending");
  });
  it("a later explicit Retry fences an older dispatch even in the same tab (cas-9dc6 recovery)", async () => {
    const { db } = journals();
    let reads = 0, writes = 0;
    let entered!: () => void, resume!: () => void;
    const atFinalCheck = new Promise<void>(resolve => { entered = resolve; });
    const wait = new Promise<void>(resolve => { resume = resolve; });
    const a = new CommanderJournal(db, async () => {
      if (++reads === 2) { entered(); await wait; }
      return fence;
    }, () => 1_000, false);
    const item = send("shared");
    await a.reconcile(scope, [], [item], fence);
    const oldDispatch = a.dispatch(scope, item.id, fence, () => { writes++; return true; });
    await atFinalCheck;
    const claimed = await a.read(scope);
    await a.reconcile(scope, claimed.sends, claimed.sends.map(row => ({ ...row, state: "unconfirmed" })), fence);
    expect(await a.retry(scope, item.id, fence, item)).toBe("kept");
    expect(await a.dispatch(scope, item.id, fence, () => { writes++; return true; })).toBe("written");
    resume();
    await oldDispatch;
    expect(writes).toBe(1);
  });
  it("a tab crash after dispatch leaves the same client_ref unconfirmed and never automatically replays", async () => {
    const { a, make } = journals();
    await a.reconcile(scope, [], [send("a")], fence);
    await a.dispatch(scope, "a", fence, () => true);
    const reload = make();
    let writes = 0;
    expect(await reload.dispatch(scope, "a", fence, () => { writes++; return true; })).toBe("unconfirmed");
    expect(writes).toBe(0);
  });
  it("a definite no-write may be held again; an exception remains uncertain", async () => {
    const { a, b } = journals();
    await a.reconcile(scope, [], [send("a"), send("b")], fence);
    expect(await a.dispatch(scope, "a", fence, () => false)).toBe("waiting");
    expect(await b.dispatch(scope, "a", fence, () => true)).toBe("written");
    expect(await a.dispatch(scope, "b", fence, () => { throw Error("uncertain socket outcome"); })).toBe("unconfirmed");
    expect(await b.dispatch(scope, "b", fence, () => true)).toBe("unconfirmed");
  });
  it("cancellation tombstones prevent a stale tab from resurrecting a held send", async () => {
    const { a, b } = journals();
    const item = send("a");
    await a.reconcile(scope, [], [item], fence);
    await b.read(scope);
    await a.reconcile(scope, [item], [], fence);
    await b.reconcile(scope, [item], [{ ...item, text: "stale edit" }], fence);
    expect((await b.read(scope)).sends).toEqual([]);
    expect(await b.dispatch(scope, "a", fence, () => true)).toBe("unconfirmed");
  });
  it("cancellation and dispatch race atomically: cancelling never reports success after a wire claim", async () => {
    const { a, b } = journals();
    await a.reconcile(scope, [], [send("a")], fence);
    let writes = 0;
    const [cancelled] = await Promise.all([b.cancel(scope, "a"), a.dispatch(scope, "a", fence, () => { writes++; return true; })]);
    expect(writes).toBe(cancelled ? 0 : 1);
    expect(await b.dispatch(scope, "a", fence, () => { writes++; return true; })).toBe("unconfirmed");
    expect(writes).toBeLessThanOrEqual(1);
  });
  it("a foreign tab cannot re-hold a claimed item even after observing its latest revision", async () => {
    const { a, b } = journals();
    await a.reconcile(scope, [], [send("a")], fence);
    await a.dispatch(scope, "a", fence, () => true);
    const rows = (await b.read(scope)).sends;
    await b.reconcile(scope, rows, [{ ...rows[0], state: "held" }], fence);
    expect((await b.read(scope)).sends[0].state).toBe("sending");
  });
  it("expiry persists as not-sent and cannot dispatch on reload", async () => {
    let now = 1_000;
    const { a, make } = journals(() => now);
    await a.reconcile(scope, [], [send("a")], fence);
    now += 120_000;
    let writes = 0;
    await a.dispatch(scope, "a", fence, () => { writes++; return true; });
    expect((await make().read(scope)).sends[0].state).toBe("error");
    expect(writes).toBe(0);
  });
  it("a credential generation change after claim fences the final wire write", async () => {
    const { db } = journals();
    let reads = 0;
    const a = new CommanderJournal(db, async () => ++reads === 1 ? fence : { ...fence, generation: 2 }, () => 1_000, false);
    await a.reconcile(scope, [], [send("a")], fence);
    let writes = 0;
    expect(await a.dispatch(scope, "a", fence, () => { writes++; return true; })).toBe("stale");
    expect(writes).toBe(0);
    expect((await a.read(scope)).sends[0].state).toBe("sending");
  });
  it("a receipt committed while the final credential check awaits prevents the retry wire write (cas-9dc6)", async () => {
    const { db, b } = journals();
    let reads = 0, writes = 0;
    const a = new CommanderJournal(db, async () => {
      if (++reads === 2) await b.acknowledge(scope, { client_ref: 'a', notification_id: 99, target: 'supervisor', stamped: true }, fence);
      return fence;
    }, () => 1_000, false);
    await a.reconcile(scope, [], [send('a')], fence);
    await a.dispatch(scope, 'a', fence, () => { writes++; return true; });
    expect(writes).toBe(0);
    expect((await a.read(scope)).receipts[0].notification_id).toBe(99);
  });
  it("an owner persisting its unconfirmed caption during the final check does not discard the wire claim (cas-9dc6)", async () => {
    const { db } = journals();
    let reads = 0, writes = 0;
    const a = new CommanderJournal(db, async () => {
      if (++reads === 2) {
        const snapshot = await a.read(scope);
        await a.reconcile(scope, snapshot.sends, snapshot.sends.map(send => ({ ...send, state: 'unconfirmed' })), fence);
      }
      return fence;
    }, () => 1_000, false);
    await a.reconcile(scope, [], [send('a')], fence);
    expect(await a.dispatch(scope, 'a', fence, () => { writes++; return true; })).toBe('written');
    expect(writes).toBe(1);
  });
  it("revocation purges private content and fences stale writers, imports and ACKs", async () => {
    const { a, b } = journals();
    await a.reconcile(scope, [], [send("a")], fence);
    await a.persistReply(scope, reply, fence);
    await a.purge(scope.hub, fence);
    await b.reconcile(scope, [], [send("b")], fence);
    await b.importLegacy(scope, [send("c")], fence);
    expect(await b.persistReply(scope, reply, fence)).toBe(false);
    expect(await b.read(scope)).toEqual({ sends: [], replies: [], receipts: [] });
  });
  it("device and origin scopes cannot read another installation's words", async () => {
    const { a } = journals();
    await a.reconcile(scope, [], [send("a")], fence);
    await a.persistReply(scope, reply, fence);
    expect(await a.read({ ...scope, device: "new-key-device" })).toEqual({ sends: [], replies: [], receipts: [] });
    expect(await a.read({ ...scope, baseUrl: "https://another.example" })).toEqual({ sends: [], replies: [], receipts: [] });
  });
  it("a committed reply is durable before ACK authorization and dedupes after lost ACK", async () => {
    const { a, make } = journals();
    expect(await a.persistReply(scope, reply, fence)).toBe(true);
    const reload = make();
    expect((await reload.read(scope)).replies[0].reply.message).toBe("Reply");
    expect(await reload.persistReply(scope, reply, fence)).toBe(true);
    expect(await reload.persistReply(scope, { ...reply, message: "conflicting replay" }, fence)).toBe(false);
    const stored = (await reload.read(scope)).replies;
    expect(stored).toHaveLength(1);
    expect(stored[0].reply.message).toBe("Reply");
  });
  it("storage failure and oversized content authorize neither send nor application ACK", async () => {
    const a = new CommanderJournal(undefined, async () => fence, () => 1_000, false);
    expect(await a.reconcile(scope, [], [send("a")], fence)).toBe("not-saved");
    expect(await a.dispatch(scope, "a", fence, () => { throw Error("must not write"); })).toBe("not-saved");
    expect(await a.persistReply(scope, reply, fence)).toBe(false);
    const { b } = journals();
    expect(await b.reconcile(scope, [], [{ ...send("a"), text: "x".repeat(65_000) }], fence)).toBe("too-long");
    expect(await b.persistReply(scope, { ...reply, message: "x".repeat(65_000) }, fence)).toBe(false);
  });
  it("legacy import is commit-once, converting on-wire state to unconfirmed", async () => {
    const { a, b } = journals();
    const item = { ...send("a"), state: "sending" as const };
    await Promise.all([a.importLegacy(scope, [item], fence), b.importLegacy(scope, [item], fence)]);
    const rows = (await a.read(scope)).sends;
    expect(rows).toHaveLength(1);
    expect(rows[0].state).toBe("unconfirmed");
    await a.reconcile(scope, rows, [], fence);
    await b.importLegacy(scope, [item], fence);
    expect((await a.read(scope)).sends).toEqual([]);
  });
});

async function seedRows(factory: IDBFactory, rows: { sends?: unknown[]; replies?: unknown[] }) {
  const db = await new Promise<IDBDatabase>((resolve) => { const open = factory.open(DELIVERY_DB, 1); open.onsuccess = () => resolve(open.result); });
  await new Promise<void>((resolve, reject) => {
    const tx = db.transaction(["sends", "replies"], "readwrite");
    for (const name of ["sends", "replies"] as const) for (const row of rows[name] ?? []) tx.objectStore(name).put(row);
    tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
  }); db.close();
}

describe("journal privacy and immutable replay", () => {
  it("drops malformed stored rows without exposing or dispatching their payload", async () => {
    const { a, db } = journals(); await a.read(scope);
    await seedRows(db, { sends: [{ key: "corrupt", send: send("corrupt"), revision: 1, updatedAt: 1_000 }], replies: [{ key: "bad-reply", scope, reply: { ...reply, message: 1 }, persistedAt: 1_000 }] });
    expect(await a.read(scope)).toEqual({ sends: [], replies: [], receipts: [] });
    expect(await a.scopes({ id: scope.hub, baseUrl: scope.baseUrl, deviceId: scope.device } as never)).toEqual([]);
  });
  it("deletes private payloads past retention when a quiet journal is reopened", async () => {
    let now = 1_000; const { a, make } = journals(() => now);
    await a.reconcile(scope, [], [send("old")], fence); await a.persistReply(scope, reply, fence);
    now += 91 * 24 * 60 * 60 * 1_000;
    expect(await make().read(scope)).toEqual({ sends: [], replies: [], receipts: [] });
  });
  it("live/history defaults dedupe but changed attachments cannot authorize an ACK", async () => {
    const { a } = journals(); expect(await a.persistReply(scope, reply, fence)).toBe(true);
    expect(await a.persistReply(scope, { ...reply, kind: "answer", attachments: [] }, fence)).toBe(true);
    expect(await a.persistReply(scope, { ...reply, attachments: [{ artifact_id: "art-other", name: "changed", mime: "text/plain", size_bytes: 1, sha256: "a".repeat(64) }] }, fence)).toBe(false);
    expect((await a.read(scope)).replies).toHaveLength(1);
  });
  it("keeps notices out of the reply journal, including resolution frames (cas-b113)", async () => {
    const { a, make } = journals();
    for (const resolved of [false, true]) {
      expect(await a.persistReply(scope, { ...reply, notice: { source: "relay-watchdog", subject: 7, resolved } }, fence)).toBe(false);
    }
    expect(await a.persistReply(scope, reply, fence)).toBe(true);
    expect((await make().read(scope)).replies.map(row => row.reply.message)).toEqual(["Reply"]);
  });
  it("removes old journaled notices before restore or cross-tab synchronization (cas-b113)", async () => {
    const { a, make, db } = journals();
    await a.persistReply(scope, reply, fence);
    const oldRows = [false, true].map((resolved, i) => {
      const notification_id = 100 + i;
      return { key: JSON.stringify([JSON.stringify([scope.hub, scope.baseUrl, scope.device, scope.session]), notification_id]), scope,
        reply: { ...reply, notification_id, message: "Never reached it", kind: "blocker", notice: { source: "relay-watchdog", subject: 7, resolved } }, persistedAt: 1_000 };
    });
    await seedRows(db, { replies: oldRows });
    const reload = make();
    expect((await reload.read(scope)).replies.map(row => row.reply.message)).toEqual(["Reply"]);
    expect((await a.read(scope)).replies).toHaveLength(1);
    // Scope discovery is another entry point before the thread restores.
    await seedRows(db, { replies: oldRows.map(row => ({ ...row, scope: { ...scope, session: "notice-only" },
      key: JSON.stringify([JSON.stringify([scope.hub, scope.baseUrl, scope.device, "notice-only"]), row.reply.notification_id]) })) });
    expect(await reload.scopes({ id: scope.hub, baseUrl: scope.baseUrl, deviceId: scope.device } as never)).toEqual([scope]);
    const request = db.open(DELIVERY_DB, 1);
    const records = await new Promise<unknown[]>(resolve => { request.onsuccess = () => {
      const database = request.result, tx = database.transaction("replies", "readonly"), read = tx.objectStore("replies").getAll();
      tx.oncomplete = () => { database.close(); resolve(read.result); };
    }; });
    expect(JSON.stringify(records)).not.toContain("Never reached it");
  });
  it("the newly committed reply remains durable at the cap even when all timestamps tie", async () => {
    const { a } = journals();
    for (let id = 101; id <= 500; id++) expect(await a.persistReply(scope, { ...reply, notification_id: id }, fence)).toBe(true);
    expect(await a.persistReply(scope, { ...reply, notification_id: 9 }, fence)).toBe(true);
    const rows = (await a.read(scope)).replies;
    expect(rows).toHaveLength(400); expect(rows.some(row => row.reply.notification_id === 9)).toBe(true);
  });
  it("future account enrollment fails closed until the enrolled journal contract exists", async () => {
    const { a } = journals();
    const machine = { id: scope.hub, baseUrl: scope.baseUrl, deviceId: scope.device, accountEnrollment: { state: "enrolled", account_id: "next-account" } };
    const future = deliveryScope(machine as never, scope.session);
    expect(await a.reconcile(future, [], [send("future")], fence)).toBe("not-saved");
    expect(await a.persistReply(future, reply, fence)).toBe(false);
    expect(await a.dispatch(future, "future", fence, () => { throw Error("must not send"); })).toBe("stale");
    expect(await a.read(future)).toEqual({ sends: [], replies: [], receipts: [] });
  });
});
