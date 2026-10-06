import { describe, expect, it } from "vitest";
import { OperatorCloudDouble } from "../../test/operator-cloud-double";
import { OperatorInboxController } from "./controller";
import { MemoryInboxStore } from "./store";

const PAGE_ORIGIN = "https://hub.petrastella.io";

function controller(cloud: OperatorCloudDouble, store = new MemoryInboxStore(), locks: ConstructorParameters<typeof OperatorInboxController>[0]["locks"] = null) {
  return new OperatorInboxController({ origin: cloud.baseUrl, store, pageOrigin: PAGE_ORIGIN, fetch: cloud.fetchFor(PAGE_ORIGIN), locks });
}

async function signIn(cloud: OperatorCloudDouble, inbox: OperatorInboxController) {
  await inbox.load();
  const state = await inbox.beginSignIn("phone");
  if (state.kind !== "awaiting_approval") throw new Error("expected a code");
  expect(await inbox.pollSignIn()).toBe(5000);
  await cloud.approve(state.userCode);
  expect(await inbox.pollSignIn()).toBeNull();
  expect(inbox.current().kind).toBe("ready");
}

describe("operator inbox controller", () => {
  it("keeps a pending sign-in code across a reload and finishes it", async () => {
    const cloud = new OperatorCloudDouble();
    const store = new MemoryInboxStore();
    const first = controller(cloud, store);
    await first.load();
    const state = await first.beginSignIn("phone");
    const reloaded = controller(cloud, store);
    expect(await reloaded.load()).toEqual(state);
    if (state.kind !== "awaiting_approval") throw new Error("expected a code");
    await cloud.approve(state.userCode);
    await reloaded.pollSignIn();
    expect(reloaded.current().kind).toBe("ready");
    expect((await controller(cloud, store).load()).kind).toBe("ready");
  });

  it("replays history with no hub involved and notifies subscribers", async () => {
    const cloud = new OperatorCloudDouble();
    await cloud.appendSessionEvent({ hubId: "h", projectId: "p", sessionId: "s", plaintext: { type: "cas.operator.turn", v: 1, snapshot: { prompt_id: 1 } } });
    const inbox = controller(cloud);
    await signIn(cloud, inbox);
    const seen: number[] = [];
    inbox.subscribe((snapshot) => seen.push(snapshot.events.length));
    expect(await inbox.runOnce()).toBe(5000);
    expect(seen.at(-1)).toBe(1);
  });

  it("wipes this profile's inbox when the device is revoked", async () => {
    const cloud = new OperatorCloudDouble();
    const store = new MemoryInboxStore();
    const inbox = controller(cloud, store);
    await signIn(cloud, inbox);
    await cloud.appendSessionEvent({ hubId: "h", projectId: "p", sessionId: "s", plaintext: { x: 1 } });
    await inbox.runOnce();
    const accountId = store.identity!.accountId;
    expect(await store.events(accountId, "1")).toHaveLength(1);
    await cloud.revokeDevice(store.identity!.deviceId);
    await inbox.runOnce();
    expect(inbox.current()).toMatchObject({ kind: "revoked" });
    expect(store.identity).toBeNull();
    expect(await store.events(accountId, "1")).toHaveLength(0);
    expect(await store.epochKey(accountId, "1", "1")).toBeNull();
  });

  it("warns once on an account reset and continues on the new generation", async () => {
    const cloud = new OperatorCloudDouble();
    const inbox = controller(cloud);
    await signIn(cloud, inbox);
    await inbox.runOnce();
    await cloud.resetFeed();
    await cloud.appendSessionEvent({ hubId: "h", projectId: "p", sessionId: "s", plaintext: { after: "reset" } });
    expect(await inbox.runOnce()).toBe(0);
    expect((await inbox.snapshot()).generationWarning).toMatch(/reset/);
    await inbox.runOnce();
    const snapshot = await inbox.snapshot();
    expect(snapshot.events.map((event) => [event.feedGeneration, event.plaintext])).toEqual([["2", { after: "reset" }]]);
  });

  it("leaves the round to the tab holding the replay lock", async () => {
    const cloud = new OperatorCloudDouble();
    const busy = { request: async (_name: string, _options: unknown, callback: (lock: Lock | null) => Promise<void>) => callback(null) };
    const inbox = controller(cloud, new MemoryInboxStore(), busy as unknown as Pick<LockManager, "request">);
    await signIn(cloud, inbox);
    await cloud.appendSessionEvent({ hubId: "h", projectId: "p", sessionId: "s", plaintext: { x: 1 } });
    await inbox.runOnce();
    expect((await inbox.snapshot()).events).toHaveLength(0);
    expect(cloud.log.some((entry) => entry.path.startsWith("/api/operator/feed?"))).toBe(false);
  });
});
