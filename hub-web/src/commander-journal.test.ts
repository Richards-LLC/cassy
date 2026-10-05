import { IDBFactory } from "fake-indexeddb";
import { describe, expect, it } from "vitest";
import { CommanderJournal, type CredentialFence, type DeliveryScope } from "./commander-journal";
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
