import { describe, expect, it } from "vitest";
import { OperatorCloudDouble } from "../../test/operator-cloud-double";
import { OperatorInboxController } from "./controller";
import { MemoryInboxStore } from "./store";
import { exportPublicKey, generateKeyPair } from "./hpke";

const PAGE_ORIGIN = "https://hub.petrastella.io";

function controller(cloud: OperatorCloudDouble, store = new MemoryInboxStore(), locks: ConstructorParameters<typeof OperatorInboxController>[0]["locks"] = null) {
  return new OperatorInboxController({ origin: cloud.baseUrl, store, pageOrigin: PAGE_ORIGIN, fetch: cloud.fetchFor(PAGE_ORIGIN), locks });
}

async function signIn(cloud: OperatorCloudDouble, inbox: OperatorInboxController, capabilities?: string[]) {
  await inbox.load();
  const state = await inbox.beginSignIn("phone");
  if (state.kind !== "awaiting_approval") throw new Error("expected a code");
  expect(await inbox.pollSignIn()).toBe(5000);
  await cloud.approve(state.userCode, capabilities ? { capabilities } : {});
  expect(await inbox.pollSignIn()).toBeNull();
  expect(inbox.current().kind).toBe("ready");
}

describe("operator inbox controller", () => {
  it("offers monitoring only to account-management devices and fences concurrent decisions", async () => {
    const cloud = new OperatorCloudDouble();
    const machine = await cloud.enrollMachine("hub", [], await exportPublicKey((await generateKeyPair()).publicKey));
    cloud.presence.set(machine.id, { monitoring: "disabled", monitoring_generation: "0" });
    const reader = controller(cloud);
    await signIn(cloud, reader);
    await reader.refreshPresence();
    expect(reader.canManageMonitoring()).toBe(false);
    await expect(reader.setMonitoring(machine.id, true)).rejects.toThrow(/cannot change/);
    expect(cloud.log.filter((r) => r.path.endsWith("/monitoring"))).toHaveLength(0);

    const manager = controller(cloud);
    await signIn(cloud, manager, ["feed:read", "account:manage"]);
    await manager.refreshPresence();
    expect(manager.canManageMonitoring()).toBe(true);
    await manager.setMonitoring(machine.id, true);
    expect((await manager.snapshot()).presence?.machines[0]).toMatchObject({ monitoring: "enabled", monitoringGeneration: "1", presence: "pending_first_report" });
    // Another authority changes the generation; the stale browser must not
    // silently disable a newer decision or retry its CAS with a new body.
    cloud.presence.set(machine.id, { monitoring: "enabled", monitoring_generation: "2", presence: "pending_first_report" });
    await expect(manager.setMonitoring(machine.id, false)).rejects.toMatchObject({ code: "monitoring_generation_conflict" });
    expect((await manager.snapshot()).presence?.machines[0].monitoringGeneration).toBe("2");
  });

  it("retries an unanswered consent with the identical decision body", async () => {
    const cloud = new OperatorCloudDouble();
    const machine = await cloud.enrollMachine("hub", [], await exportPublicKey((await generateKeyPair()).publicKey));
    cloud.presence.set(machine.id, { monitoring: "disabled", monitoring_generation: "0" });
    const transport = cloud.fetchFor(PAGE_ORIGIN);
    const bodies: string[] = [];
    const inbox = new OperatorInboxController({ origin: cloud.baseUrl, store: new MemoryInboxStore(), pageOrigin: PAGE_ORIGIN, fetch: async (input, init) => {
      const result = await transport(input, init);
      if (String(input).endsWith("/monitoring")) {
        bodies.push(new TextDecoder().decode(init!.body as Uint8Array));
        if (bodies.length === 1) throw new TypeError("response lost after commit");
      }
      return result;
    } });
    await signIn(cloud, inbox, ["feed:read", "account:manage"]);
    await inbox.refreshPresence();
    await expect(inbox.setMonitoring(machine.id, true)).rejects.toMatchObject({ status: 0 });
    expect((await inbox.snapshot()).presence?.machines[0].monitoring).toBe("disabled");
    await inbox.setMonitoring(machine.id, true);
    expect(bodies).toHaveLength(2);
    expect(bodies[0]).toBe(bodies[1]);
    expect((await inbox.snapshot()).presence?.machines[0].monitoringGeneration).toBe("1");
  });

  it("stores verified outage and matching recovery on a separate device with no hub involved", async () => {
    const cloud = new OperatorCloudDouble();
    const machine = await cloud.enrollMachine("hub", [], await exportPublicKey((await generateKeyPair()).publicKey));
    const phoneStore = new MemoryInboxStore();
    const phone = controller(cloud, phoneStore);
    await signIn(cloud, phone);
    const outage = await cloud.appendObserverNotice(machine, "machine_unobserved");
    await phone.runOnce();
    await cloud.appendObserverNotice(machine, "machine_recovered", { refEventId: outage.eventId });
    await phone.runOnce();
    expect((await phone.snapshot()).events).toHaveLength(2);
    expect((await phone.snapshot()).events.every((event) => event.verification === "verified" && event.acked)).toBe(true);
    const reloaded = controller(cloud, phoneStore);
    await reloaded.load();
    expect((await reloaded.snapshot()).events.map((event) => (event.plaintext as { kind: string }).kind)).toEqual(["machine_unobserved", "machine_recovered"]);
    expect(cloud.acks.get(phoneStore.identity!.deviceId)?.size).toBe(2);
  });

  it("labels stale status after a failed refresh instead of replacing it with empty/healthy", async () => {
    const cloud = new OperatorCloudDouble();
    const machine = await cloud.enrollMachine("hub", [], await exportPublicKey((await generateKeyPair()).publicKey));
    cloud.presence.set(machine.id, { monitoring: "enabled", monitoring_generation: "1", presence: "unobserved" });
    const transport = cloud.fetchFor(PAGE_ORIGIN);
    let fail = false;
    const inbox = new OperatorInboxController({ origin: cloud.baseUrl, store: new MemoryInboxStore(), pageOrigin: PAGE_ORIGIN, fetch: (input, init) => {
      if (fail && String(input).endsWith("/machine-presence")) throw new TypeError("offline");
      return transport(input, init);
    } });
    await signIn(cloud, inbox);
    await inbox.refreshPresence();
    fail = true;
    await inbox.refreshPresence(true);
    expect((await inbox.snapshot()).presence?.machines[0].presence).toBe("unobserved");
    expect((await inbox.snapshot()).presenceError).toMatch(/earlier check/);
  });
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
