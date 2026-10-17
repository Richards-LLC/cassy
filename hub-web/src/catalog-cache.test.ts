// @vitest-environment jsdom
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
  it("keeps each machine's last catalog across a reload, without browser-only flags, until it empties", () => {
    const storage = memoryStorage();
    catalogCacheStore(storage).save("atlas", [session("cas-src", { unreachable: true, last_activity_at: "2026-10-16T15:00:00Z" })]);
    const reloaded = catalogCacheStore(storage);
    expect(reloaded.load("atlas")).toEqual([
      { name: "cas-src", supervisor: "supervisor", workers: ["worker-1"], liveness: "live", project_dir: "/repo/cas-src", ws_port: 4100, last_activity_at: "2026-10-16T15:00:00Z" },
    ]);
    expect(reloaded.load("pelican")).toBeUndefined();
    // An empty catalog removes the machine's entry.
    reloaded.save("atlas", []);
    expect(reloaded.load("atlas")).toBeUndefined();
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

describe("footer count before any machine answers (cas-9846)", () => {
  it("counts a kept catalog as last known, never as not loaded or zero", async () => {
    const { machineFooterMarkup } = await import("./paired-machines");
    const meta = (markup: string) => { const node = document.createElement("div"); node.innerHTML = markup; return node.querySelector(".hub-footer-meta span")?.textContent; };
    // pin: the footer's words for each catalog state.
    expect(meta(machineFooterMarkup([], undefined, "b"))).toBe("Conversations not loaded yet");
    expect(meta(machineFooterMarkup([], 1, "b", false, true))).toBe("1 conversation, last known");
    expect(meta(machineFooterMarkup([], 3, "b", false, true))).toBe("3 conversations, last known");
    expect(meta(machineFooterMarkup([], 3, "b"))).toBe("3 conversations");
  });
});
