import { describe, expect, it } from "vitest";
import { catalogCacheStore, CONVERSATION_STORE_PREFIX, purgeConversations, validCachedSession } from "./conversation-store";
import type { HubSession } from "./types";

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

const session = (name: string, extra: Partial<HubSession> = {}): HubSession => ({
  name, supervisor: "supervisor", workers: ["worker-1"], liveness: "live", project_dir: "/repo/cas-src", ws_port: 4100, ...extra,
});

describe("cached session catalog (cas-9846)", () => {
  it("keeps each machine's last catalog across a reload, without browser-only flags", () => {
    const storage = memoryStorage();
    catalogCacheStore(storage).save("atlas", [session("cas-src", { unreachable: true, last_activity_at: "2026-10-16T15:00:00Z" })]);
    const reloaded = catalogCacheStore(storage);
    expect(reloaded.load("atlas")).toEqual([
      { name: "cas-src", supervisor: "supervisor", workers: ["worker-1"], liveness: "live", project_dir: "/repo/cas-src", ws_port: 4100, last_activity_at: "2026-10-16T15:00:00Z" },
    ]);
    expect(reloaded.load("pelican")).toBeUndefined();
  });

  it("an empty catalog removes the machine's entry", () => {
    const storage = memoryStorage();
    const store = catalogCacheStore(storage);
    store.save("atlas", [session("cas-src")]);
    store.save("atlas", []);
    expect(store.load("atlas")).toBeUndefined();
  });

  it("is purged with the machine's pairing, like every conversation namespace", () => {
    const storage = memoryStorage();
    const store = catalogCacheStore(storage);
    store.save("atlas", [session("cas-src")]);
    store.save("pelican", [session("gabber")]);
    purgeConversations(storage, "atlas");
    expect(store.load("atlas")).toBeUndefined();
    expect(store.load("pelican")?.map((row) => row.name)).toEqual(["gabber"]);
  });

  it("drops a planted catalog with a row this page could not have written", () => {
    const key = `${CONVERSATION_STORE_PREFIX}:catalog:v1`;
    const storage = memoryStorage({
      [key]: JSON.stringify({ "atlas:catalog": { value: [session("cas-src"), { name: "x", supervisor: "s", workers: [1], liveness: "live" }], updatedAt: 1 } }),
    });
    expect(catalogCacheStore(storage).load("atlas")).toBeUndefined();
    expect(validCachedSession({ name: "x", supervisor: "s", workers: [], liveness: "exploded" })).toBeUndefined();
  });

  it("missing or refusing storage never breaks the page", () => {
    expect(catalogCacheStore(undefined).load("atlas")).toBeUndefined();
    const denied = { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("denied"); }, removeItem: () => undefined };
    const store = catalogCacheStore(denied);
    expect(() => store.save("atlas", [session("cas-src")])).not.toThrow();
    expect(store.load("atlas")).toBeUndefined();
  });
});
