import { IDBFactory } from "fake-indexeddb";
import { describe, expect, it } from "vitest";
import { CommanderJournal, deliveryScope, DELIVERY_DB, type CredentialFence, type DeliveryScope } from "./commander-journal";
import type { PendingSend } from "./conversation-store";

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
  it("revocation purges private content and fences stale writers, imports and ACKs", async () => {
    const { a, b } = journals();
    await a.reconcile(scope, [], [send("a")], fence);
    await a.persistReply(scope, reply, fence);
    await a.purge(scope.hub, fence);
    await b.reconcile(scope, [], [send("b")], fence);
    await b.importLegacy(scope, [send("c")], fence);
    expect(await b.persistReply(scope, reply, fence)).toBe(false);
    expect(await b.read(scope)).toEqual({ sends: [], replies: [] });
  });
  it("device and origin scopes cannot read another installation's words", async () => {
    const { a } = journals();
    await a.reconcile(scope, [], [send("a")], fence);
    await a.persistReply(scope, reply, fence);
    expect(await a.read({ ...scope, device: "new-key-device" })).toEqual({ sends: [], replies: [] });
    expect(await a.read({ ...scope, baseUrl: "https://another.example" })).toEqual({ sends: [], replies: [] });
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
    expect(await a.read(scope)).toEqual({ sends: [], replies: [] });
    expect(await a.scopes({ id: scope.hub, baseUrl: scope.baseUrl, deviceId: scope.device } as never)).toEqual([]);
  });
  it("deletes private payloads past retention when a quiet journal is reopened", async () => {
    let now = 1_000; const { a, make } = journals(() => now);
    await a.reconcile(scope, [], [send("old")], fence); await a.persistReply(scope, reply, fence);
    now += 91 * 24 * 60 * 60 * 1_000;
    expect(await make().read(scope)).toEqual({ sends: [], replies: [] });
  });
  it("live/history defaults dedupe but changed attachments cannot authorize an ACK", async () => {
    const { a } = journals(); expect(await a.persistReply(scope, reply, fence)).toBe(true);
    expect(await a.persistReply(scope, { ...reply, kind: "answer", attachments: [] }, fence)).toBe(true);
    expect(await a.persistReply(scope, { ...reply, attachments: [{ artifact_id: "art-other", name: "changed", mime: "text/plain", size_bytes: 1, sha256: "a".repeat(64) }] }, fence)).toBe(false);
    expect((await a.read(scope)).replies).toHaveLength(1);
  });
  it("notice replay canonicalizes defaults and field order without storing unknown fields", async () => {
    const { a } = journals();
    expect(await a.persistReply(scope, { ...reply, notice: { source: "relay-watchdog", subject: 7, private_extra: "discard" } } as never, fence)).toBe(true);
    expect((await a.read(scope)).replies[0].reply.notice).toEqual({ source: "relay-watchdog", subject: 7, resolved: false });
    expect(await a.persistReply(scope, { ...reply, notice: { resolved: false, subject: 7, source: "relay-watchdog" } }, fence)).toBe(true);
    expect(await a.persistReply(scope, { ...reply, notice: { source: "relay-watchdog", subject: 7, resolved: true } }, fence)).toBe(false);
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
    expect(await a.read(future)).toEqual({ sends: [], replies: [] });
  });
});
