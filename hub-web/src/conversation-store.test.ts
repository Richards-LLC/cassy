import { describe, expect, it } from "vitest";
import { ConversationStore, draftStore, purgeConversations, validDraft } from "./conversation-store";

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
