import { describe, expect, it, vi } from "vitest";
import { arrivalStore, readMarkStore, ConversationStore, draftStore, MAX_ARRIVALS, MAX_PENDING_SENDS, PENDING_SEND_BOUNDS, pendingSendStore, purgeConversations, validArrivals, validDraft, validPendingSend, validPendingSends, type PendingSend } from "./conversation-store";

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    values,
    get length() { return values.size; },
    key: (index: number) => [...values.keys()][index] ?? null,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
    removeItem: (key: string) => { values.delete(key); },
  };
}
const text = (raw: unknown) => (typeof raw === "string" ? raw : undefined);

describe("conversation store (cas-7752, shared with cas-e7b1)", () => {
  it("keeps each conversation's value apart, keyed by machine and session", () => {
    const storage = memoryStorage();
    const store = new ConversationStore(storage, "test", text);
    store.set("atlas:patient-pelican-9", "one");
    store.set("studio:patient-pelican-9", "two");
    const reloaded = new ConversationStore(storage, "test", text);
    expect(reloaded.get("atlas:patient-pelican-9")).toBe("one");
    expect(reloaded.get("studio:patient-pelican-9")).toBe("two");
    expect(reloaded.get("atlas:other-session")).toBeUndefined();
  });

  it("namespaces do not see each other", () => {
    const storage = memoryStorage();
    new ConversationStore(storage, "drafts", text).set("a:x", "draft");
    expect(new ConversationStore(storage, "held", text).get("a:x")).toBeUndefined();
  });

  it("is bounded: the least recently written conversations go first, and oversized values are not kept", () => {
    const storage = memoryStorage();
    let clock = 0;
    const store = new ConversationStore(storage, "test", text, { maxConversations: 2, maxValueChars: 20 }, () => ++clock);
    store.set("a:1", "first");
    store.set("a:2", "second");
    store.set("a:1", "first again");
    store.set("a:3", "third");
    expect([...store.entries().keys()]).toEqual(["a:1", "a:3"]);
    store.set("a:1", "x".repeat(50));
    expect(store.get("a:1"), "an oversized value removes the entry rather than keeping a stale one").toBeUndefined();
  });

  it("deleting the last conversation removes the storage entry", () => {
    const storage = memoryStorage();
    const store = new ConversationStore(storage, "test", text);
    store.set("a:x", "v");
    store.delete("a:x");
    expect(storage.values.has(store.key)).toBe(false);
  });

  it("unreadable, foreign or failing storage yields nothing and never throws", () => {
    for (const raw of ["{not json", "[]", "null", JSON.stringify({ "a:x": "not a record" }), JSON.stringify({ "a:x": { value: 3 } })]) {
      const storage = memoryStorage({ "cas-commander-conversation:test:v1": raw });
      expect(new ConversationStore(storage, "test", text).entries().size, raw).toBe(0);
    }
    expect(new ConversationStore(undefined, "test", text).entries().size).toBe(0);
    const failing = { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("quota"); }, removeItem: () => { throw new Error("quota"); } };
    const store = new ConversationStore(failing, "test", text);
    expect(() => store.set("a:x", "v")).not.toThrow();
    expect(() => store.delete("a:x")).not.toThrow();
    expect(store.get("a:x")).toBeUndefined();
  });
});

describe("stored conversation cleanup on load", () => {
  const key = "cas-commander-conversation:test:v1";
  const valid = { value: "Keep this conversation", updatedAt: 123 };

  it("removes a planted 2 MB conversation and malformed entries in one rewrite, preserving the valid neighbour", () => {
    const storage = memoryStorage({ [key]: JSON.stringify({
      "atlas:planted": { value: "x".repeat(2_000_000), updatedAt: 200 },
      "atlas:broken": null,
      "atlas:invalid": { value: 7, updatedAt: 100 },
      "atlas:valid": valid,
    }) });
    const set = vi.spyOn(storage, "setItem");
    const remove = vi.spyOn(storage, "removeItem");
    const store = new ConversationStore(storage, "test", text);
    expect([...store.entries()]).toEqual([["atlas:valid", valid.value]]);
    expect(JSON.parse(storage.getItem(key)!)).toEqual({ "atlas:valid": valid });
    expect(set).toHaveBeenCalledTimes(1);
    expect(remove).not.toHaveBeenCalled();
    expect(store.get("atlas:valid")).toBe(valid.value);
    expect(store.entries().size).toBe(1);
    expect(set).toHaveBeenCalledTimes(1);
  });

  it.each(["{not json", "[]", "null", "3", '\"foreign\"', ""])("removes an unparseable or invalid namespace %j", (raw) => {
    const storage = memoryStorage({ [key]: raw });
    const remove = vi.spyOn(storage, "removeItem");
    const set = vi.spyOn(storage, "setItem");
    const store = new ConversationStore(storage, "test", text);
    expect(store.entries().size).toBe(0);
    expect(storage.getItem(key)).toBeNull();
    expect(store.get("atlas:valid")).toBeUndefined();
    expect(remove).toHaveBeenCalledTimes(1);
    expect(set).not.toHaveBeenCalled();
  });

  it("removes a namespace when every entry was dropped", () => {
    const storage = memoryStorage({ [key]: JSON.stringify({ "atlas:bad": { value: 3 } }) });
    const remove = vi.spyOn(storage, "removeItem");
    expect(new ConversationStore(storage, "test", text).entries().size).toBe(0);
    expect(storage.getItem(key)).toBeNull();
    expect(remove).toHaveBeenCalledTimes(1);
  });

  it.each([true, false])("never writes a valid or absent namespace (present=%s)", (present) => {
    const raw = JSON.stringify({ "atlas:valid": valid });
    const storage = memoryStorage(present ? { [key]: raw } : {});
    const set = vi.spyOn(storage, "setItem");
    const remove = vi.spyOn(storage, "removeItem");
    const store = new ConversationStore(storage, "test", text);
    store.entries();
    expect(store.get("atlas:valid")).toBe(present ? valid.value : undefined);
    store.entries();
    expect(storage.getItem(key)).toBe(present ? raw : null);
    expect(set).not.toHaveBeenCalled();
    expect(remove).not.toHaveBeenCalled();
  });

  it("tolerates a denied rewrite and attempts cleanup once per store load", () => {
    const storage = memoryStorage({ [key]: JSON.stringify({ "atlas:bad": null, "atlas:valid": valid }) });
    const set = vi.spyOn(storage, "setItem").mockImplementation(() => { throw new Error("quota"); });
    const store = new ConversationStore(storage, "test", text);
    expect(store.get("atlas:valid")).toBe(valid.value);
    expect(store.entries().size).toBe(1);
    expect(store.get("atlas:bad")).toBeUndefined();
    expect(set).toHaveBeenCalledTimes(1);
    expect(new ConversationStore(storage, "test", text).get("atlas:valid")).toBe(valid.value);
    expect(set).toHaveBeenCalledTimes(2);
  });

  it("tolerates a denied removal without retrying on every read", () => {
    const storage = memoryStorage({ [key]: "{not json" });
    const remove = vi.spyOn(storage, "removeItem").mockImplementation(() => { throw new Error("denied"); });
    const store = new ConversationStore(storage, "test", text);
    expect(store.entries().size).toBe(0);
    expect(store.get("atlas:valid")).toBeUndefined();
    expect(remove).toHaveBeenCalledTimes(1);
  });

  it("retires an unreadable namespace once and still tolerates storage denial", () => {
    const storage = memoryStorage({ [key]: "unreadable" });
    vi.spyOn(storage, "getItem").mockImplementation(() => { throw new Error("denied"); });
    const remove = vi.spyOn(storage, "removeItem");
    const store = new ConversationStore(storage, "test", text);
    expect(store.entries().size).toBe(0);
    expect(store.get("atlas:valid")).toBeUndefined();
    expect(storage.values.has(key)).toBe(false);
    expect(remove).toHaveBeenCalledTimes(1);
  });

  it("drops a value whose validator cannot read it and preserves its neighbour", () => {
    const storage = memoryStorage({ [key]: JSON.stringify({ "atlas:bad": { value: "unreadable" }, "atlas:valid": valid }) });
    const validate = (raw: unknown) => { if (raw === "unreadable") throw new Error("cannot read"); return text(raw); };
    expect(new ConversationStore(storage, "test", validate).get("atlas:valid")).toBe(valid.value);
    expect(JSON.parse(storage.getItem(key)!)).toEqual({ "atlas:valid": valid });
  });

  it("also retires the least recent conversations dropped by the load bound", () => {
    const storage = memoryStorage({ [key]: JSON.stringify({
      "atlas:old": { value: "old", updatedAt: 1 },
      "atlas:valid": valid,
    }) });
    const store = new ConversationStore(storage, "test", text, { maxConversations: 1 });
    expect([...store.entries()]).toEqual([["atlas:valid", valid.value]]);
    expect(JSON.parse(storage.getItem(key)!)).toEqual({ "atlas:valid": valid });
  });
});

describe("drafts across a reload (cas-7752)", () => {
  it("a draft comes back for its conversation with its caret, and a blank or sent draft is gone", () => {
    const storage = memoryStorage();
    draftStore(storage).save("atlas:patient-pelican-9", { text: "Draft: ask about the flaky pairing test", caret: 6 });
    expect(draftStore(storage).load().get("atlas:patient-pelican-9")).toEqual({ text: "Draft: ask about the flaky pairing test", caret: 6 });
    draftStore(storage).save("atlas:patient-pelican-9", { text: "   ", caret: 0 });
    expect(draftStore(storage).load().size).toBe(0);
    draftStore(storage).save("atlas:patient-pelican-9", { text: "again", caret: 5 });
    draftStore(storage).save("atlas:patient-pelican-9", undefined);
    expect(draftStore(storage).load().size).toBe(0);
  });

  it("says whether a draft was kept: under the bound as before, a 70k draft is too long and an older copy goes (cas-adfc)", () => {
    const storage = memoryStorage();
    const drafts = draftStore(storage);
    expect(drafts.save("atlas:pelican", { text: "short", caret: 5 })).toBe("kept");
    expect(draftStore(storage).load().get("atlas:pelican")).toEqual({ text: "short", caret: 5 });
    expect(drafts.save("atlas:pelican", { text: "z".repeat(70_000), caret: 70_000 })).toBe("too-long");
    // The shorter copy is not restored in its place: a reload shows no stale draft.
    expect(draftStore(storage).load().size).toBe(0);
    expect(drafts.save("atlas:pelican", { text: "z".repeat(1_000), caret: 0 })).toBe("kept");
    expect(draftStore(storage).load().get("atlas:pelican")?.text).toHaveLength(1_000);
    expect(drafts.save("atlas:pelican", { text: "  ", caret: 0 })).toBe("cleared");
    expect(drafts.save("atlas:pelican", undefined)).toBe("cleared");
    expect(draftStore(storage).load().size).toBe(0);
  });

  it("measures the bound on the stored form, so the edge is exact (cas-adfc)", () => {
    const storage = memoryStorage();
    const fits = (n: number) => JSON.stringify({ text: "a".repeat(n), caret: 0 }).length;
    const edge = 64_000 - fits(0);
    expect(fits(edge)).toBe(64_000);
    expect(draftStore(storage).save("a:s", { text: "a".repeat(edge), caret: 0 })).toBe("kept");
    expect(draftStore(storage).save("a:s", { text: "a".repeat(edge + 1), caret: 0 })).toBe("too-long");
    expect(new ConversationStore(storage, "t", text, { maxValueChars: 5 }).set("a:s", "abc")).toBe(true);
    expect(new ConversationStore(storage, "t", text, { maxValueChars: 5 }).set("a:s", "abcd")).toBe(false);
  });

  it("clamps a caret outside the text", () => {
    expect(validDraft({ text: "hi", caret: 99 })).toEqual({ text: "hi", caret: 2 });
    expect(validDraft({ text: "hi", caret: -3 })).toEqual({ text: "hi", caret: 0 });
    expect(validDraft({ text: "hi" })).toEqual({ text: "hi", caret: 2 });
    expect(validDraft({ text: 4 })).toBeUndefined();
  });
});

describe("purging stored conversations when a pairing goes (cas-7752)", () => {
  it("a revoked or removed machine loses its conversations in every namespace; other machines keep theirs", () => {
    const storage = memoryStorage({ "unrelated-app-key": "keep" });
    const drafts = new ConversationStore(storage, "drafts", text);
    const held = new ConversationStore(storage, "held", text);
    drafts.set("atlas:patient-pelican-9", "atlas draft");
    drafts.set("studio:calm-otter-4", "studio draft");
    held.set("atlas:patient-pelican-9", "atlas held");
    purgeConversations(storage, "atlas");
    expect(drafts.get("atlas:patient-pelican-9")).toBeUndefined();
    expect(held.get("atlas:patient-pelican-9")).toBeUndefined();
    expect(held.entries().size, "an emptied namespace is removed").toBe(0);
    expect(storage.values.has(held.key)).toBe(false);
    expect(drafts.get("studio:calm-otter-4")).toBe("studio draft");
    expect(storage.values.get("unrelated-app-key")).toBe("keep");
  });

  it("a machine id is matched whole, not as a prefix of another machine", () => {
    const storage = memoryStorage();
    const drafts = new ConversationStore(storage, "drafts", text);
    drafts.set("atlas:s", "atlas");
    drafts.set("atlas-2:s", "atlas-2");
    purgeConversations(storage, "atlas");
    expect([...drafts.entries().keys()]).toEqual(["atlas-2:s"]);
  });

  it("with no machine named, every conversation store is cleared (forget this browser)", () => {
    const storage = memoryStorage({ "unrelated-app-key": "keep" });
    new ConversationStore(storage, "drafts", text).set("atlas:s", "a");
    new ConversationStore(storage, "held", text).set("studio:s", "b");
    purgeConversations(storage);
    expect([...storage.values.keys()]).toEqual(["unrelated-app-key"]);
  });

  it("an unreadable store entry is removed rather than kept, and missing storage is a no-op", () => {
    const storage = memoryStorage({ "cas-commander-conversation:drafts:v1": "{not json" });
    purgeConversations(storage, "atlas");
    expect(storage.values.size).toBe(0);
    expect(() => purgeConversations(undefined, "atlas")).not.toThrow();
  });
});

describe("unsettled messages store (cas-e7b1)", () => {
  const send = (id: string, extra: Partial<PendingSend> = {}): PendingSend => ({ id, target: "sup", text: `message ${id}`, state: "held", at: 1, ...extra });

  it("keeps a conversation's unsettled messages in order, and an empty list removes them", () => {
    const storage = memoryStorage();
    pendingSendStore(storage).save("atlas:pelican", [send("a", { heldAt: 1, replyTo: 4, session: "pelican" }), send("b", { state: "unconfirmed", sentAt: 2 })]);
    expect(pendingSendStore(storage).load().get("atlas:pelican")).toEqual([
      { id: "a", target: "sup", text: "message a", state: "held", at: 1, heldAt: 1, replyTo: 4, session: "pelican" },
      { id: "b", target: "sup", text: "message b", state: "unconfirmed", at: 1, sentAt: 2 },
    ]);
    pendingSendStore(storage).save("atlas:pelican", []);
    expect(pendingSendStore(storage).load().size).toBe(0);
  });

  it("drops malformed messages, keeps the newest per conversation, and is purged with its machine", () => {
    const storage = memoryStorage({
      "cas-commander-conversation:sends:v1": JSON.stringify({ "atlas:s": { value: [send("ok"), { id: "no-text", target: "sup", text: " ", state: "held", at: 1 }, { ...send("bad-state"), state: "delivered" }, { ...send("no-at"), at: "soon" }], updatedAt: 1 } }),
    });
    expect(pendingSendStore(storage).load().get("atlas:s")?.map((kept) => kept.id)).toEqual(["ok"]);
    const many = Array.from({ length: MAX_PENDING_SENDS + 5 }, (_, index) => send(`m${index}`));
    pendingSendStore(storage).save("atlas:s", many);
    expect(pendingSendStore(storage).load().get("atlas:s")?.map((kept) => kept.id)).toEqual(many.slice(-MAX_PENDING_SENDS).map((kept) => kept.id));
    pendingSendStore(storage).save("studio:s", [send("studio")]);
    purgeConversations(storage, "atlas");
    expect([...pendingSendStore(storage).load().keys()]).toEqual(["studio:s"]);
  });
});

describe("stored values this page could not have written are dropped on read (cas-8f19)", () => {
  const key = "cas-commander-conversation:sends:v1";
  const held = (id: string, text: string) => ({ id, target: "sup", text, state: "held", at: 1, heldAt: 1 });

  it("a 2 MB planted waiting message is not read back, so nothing restores or sends it", () => {
    const storage = memoryStorage({
      [key]: JSON.stringify({
        "atlas:pelican": { value: [held("planted", "x".repeat(2_000_000))], updatedAt: 2 },
        "atlas:otter": { value: [held("ok", "Ship it when green")], updatedAt: 1 },
      }),
    });
    const loaded = pendingSendStore(storage).load();
    expect(loaded.has("atlas:pelican"), "the oversized conversation is dropped").toBe(false);
    expect(loaded.get("atlas:otter")?.map((send) => send.id), "a valid conversation beside it is kept").toEqual(["ok"]);
    // The same entry is refused by the shape check alone, without the store's bound.
    expect(validPendingSend(held("planted", "x".repeat(2_000_000)))).toBeUndefined();
  });

  it("checks every field's bound on read, as on write", () => {
    const ok = { id: "a", target: "sup", text: "hi", state: "error", at: 1, error: "Not sent", session: "pelican" };
    expect(validPendingSend(ok)).toBeDefined();
    for (const [field, length] of Object.entries(PENDING_SEND_BOUNDS)) {
      expect(validPendingSend({ ...ok, [field]: "y".repeat(length + 1) }), `${field} over ${length}`).toBeUndefined();
      expect(validPendingSend({ ...ok, [field]: "y".repeat(length) }), `${field} at ${length}`).toBeDefined();
    }
  });

  it("keeps at most the newest MAX_PENDING_SENDS of a planted list, and the store's conversation bound", () => {
    const many = Array.from({ length: MAX_PENDING_SENDS + 30 }, (_, index) => held(`m${index}`, `message ${index}`));
    expect(validPendingSends(many)?.map((send) => send.id)).toEqual(many.slice(-MAX_PENDING_SENDS).map((send) => send.id));
    const storage = memoryStorage({
      "cas-commander-conversation:test:v1": JSON.stringify(Object.fromEntries(Array.from({ length: 5 }, (_, index) => [`a:${index}`, { value: `v${index}`, updatedAt: index }]))),
    });
    expect([...new ConversationStore(storage, "test", text, { maxConversations: 2 }).entries().keys()]).toEqual(["a:3", "a:4"]);
  });

  it("an oversized planted draft is not restored either", () => {
    const storage = memoryStorage({ "cas-commander-conversation:drafts:v1": JSON.stringify({ "atlas:pelican": { value: { text: "z".repeat(70_000), caret: 0 }, updatedAt: 1 } }) });
    expect(draftStore(storage).load().size).toBe(0);
  });
});

describe("turn times store (cas-8d52)", () => {
  it("keeps valid times and the lead, drops the rest, and bounds the record", () => {
    expect(validArrivals({ skew: 300_000, at: { "r:1": 10, "s:2": 20, "x:3": 30, "r:4": "late", "r:5": Number.NaN } })).toEqual({ skew: 300_000, at: { "r:1": 10, "s:2": 20 } });
    expect(validArrivals({ at: {} })).toBeUndefined();
    expect(validArrivals("{not json")).toBeUndefined();
    const many = Object.fromEntries(Array.from({ length: MAX_ARRIVALS + 10 }, (_, index) => [`r:${index}`, index]));
    const kept = validArrivals({ at: many })!;
    expect(Object.keys(kept.at)).toHaveLength(MAX_ARRIVALS);
    expect(kept.at["r:0"]).toBeUndefined();
  });
  it("keeps each live turn's clock-ahead mark only for a kept supervisor turn (cas-9e33)", () => {
    expect(validArrivals({ at: { "r:1": 10, "s:2": 20 }, live: { "r:1": false, "s:2": true, "r:3": true, "r:x": true } })).toEqual({ at: { "r:1": 10, "s:2": 20 }, live: { "r:1": false } });
    expect(validArrivals({ at: { "r:1": 10 }, live: { "r:1": "yes" } })).toEqual({ at: { "r:1": 10 } });
    expect(validArrivals({ at: { "r:1": 10 }, live: ["r:1"] })).toEqual({ at: { "r:1": 10 } });
    const storage = memoryStorage();
    arrivalStore(storage).save("atlas:pelican", { at: { "r:1": 100, "r:2": 200 }, live: { "r:1": false, "r:2": true } });
    expect(arrivalStore(storage).load().get("atlas:pelican")).toEqual({ at: { "r:1": 100, "r:2": 200 }, live: { "r:1": false, "r:2": true } });
  });
  it("round-trips per conversation and is purged with its machine", () => {
    const storage = memoryStorage();
    const store = arrivalStore(storage);
    store.save("atlas:pelican", { skew: 5, at: { "r:1": 100 } });
    store.save("studio:otter", { at: { "s:2": 200 } });
    expect(arrivalStore(storage).load().get("atlas:pelican")).toEqual({ skew: 5, at: { "r:1": 100 } });
    purgeConversations(storage as never, "atlas");
    expect([...arrivalStore(storage).load().keys()]).toEqual(["studio:otter"]);
    store.save("studio:otter", { at: {} });
    expect(arrivalStore(storage).load().size).toBe(0);
  });
});

describe("a draft the browser refuses to store says so (cas-f657)", () => {
  /** A Storage with a byte quota, as a browser's localStorage is: setItem past it throws QuotaExceededError. */
  function quotaStorage(quotaChars: number) {
    const data = new Map<string, string>();
    const used = () => [...data].reduce((sum, [key, value]) => sum + key.length + value.length, 0);
    return {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => {
        const before = data.get(key);
        const next = used() - (before === undefined ? 0 : key.length + before.length) + key.length + value.length;
        if (next > quotaChars) throw new DOMException("The quota has been exceeded.", "QuotaExceededError");
        data.set(key, value);
      },
      removeItem: (key: string) => { data.delete(key); },
    };
  }

  it("reports not-saved when a full localStorage throws on the draft's write, and keeps nothing stale", () => {
    const storage = quotaStorage(8_000);
    // Another page's data fills the quota first.
    storage.setItem("other-app:cache", "x".repeat(7_900));
    const drafts = draftStore(storage);
    expect(drafts.save("atlas:pelican", { text: "a reply that will not fit on disk", caret: 0 })).toBe("not-saved");
    expect(draftStore(storage).load().size).toBe(0);
    // Room again (the other page cleared its cache): the same draft is kept.
    storage.removeItem("other-app:cache");
    expect(drafts.save("atlas:pelican", { text: "a reply that will not fit on disk", caret: 0 })).toBe("kept");
    expect(draftStore(storage).load().get("atlas:pelican")?.text).toBe("a reply that will not fit on disk");
  });

  it("still reports too-long, not not-saved, for a draft over the store's own bound", () => {
    expect(draftStore(quotaStorage(8_000)).save("atlas:pelican", { text: "z".repeat(70_000), caret: 0 })).toBe("too-long");
  });
  it("remembers the newest reply read per conversation across a reload, and drops it with its machine (cas-97d58 F14)", () => {
    const storage = memoryStorage();
    readMarkStore(storage).save("atlas:pelican", 42);
    readMarkStore(storage).save("studio:otter", 7);
    expect(readMarkStore(storage).load()).toEqual(new Map([["atlas:pelican", 42], ["studio:otter", 7]]));
    purgeConversations(storage, "atlas");
    expect(readMarkStore(storage).load()).toEqual(new Map([["studio:otter", 7]]));
  });
});
