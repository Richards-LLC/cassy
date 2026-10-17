import { createHash } from "node:crypto";
import { describe, expect, it, vi } from "vitest";
import { OperatorCloudDouble } from "../../test/operator-cloud-double";
import { OperatorInboxController } from "./controller";
import { MemoryInboxStore } from "./store";
import { exportPublicKey, generateKeyPair } from "./hpke";
import { OperatorWireError } from "./wire";

const PAGE_ORIGIN = "https://hub.petrastella.io";

function controller(cloud: OperatorCloudDouble, store = new MemoryInboxStore(), locks: ConstructorParameters<typeof OperatorInboxController>[0]["locks"] = null) {
  return new OperatorInboxController({ origin: cloud.baseUrl, store, pageOrigin: PAGE_ORIGIN, fetch: cloud.fetchFor(PAGE_ORIGIN), locks });
}

async function signIn(cloud: OperatorCloudDouble, inbox: OperatorInboxController, capabilities?: string[], label = "phone") {
  await inbox.load();
  const state = await inbox.beginSignIn(label);
  if (state.kind !== "awaiting_approval") throw new Error("expected a code");
  expect(await inbox.pollSignIn()).toBe(5000);
  await cloud.approve(state.userCode, capabilities ? { capabilities } : {});
  expect(await inbox.pollSignIn()).toBeNull();
  expect(inbox.current().kind).toBe("ready");
}

describe("operator inbox controller", () => {
  it("refreshes after a monitoring decision even when an earlier status request is in flight", async () => {
    const cloud = new OperatorCloudDouble();
    const machine = await cloud.enrollMachine("hub", [], await exportPublicKey((await generateKeyPair()).publicKey));
    cloud.presence.set(machine.id, { monitoring: "enabled", monitoring_generation: "1", presence: "pending_first_report" });
    const transport = cloud.fetchFor(PAGE_ORIGIN);
    let hold = false;
    let release!: () => void;
    let started!: () => void;
    const requested = new Promise<void>((resolve) => { started = resolve; });
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const inbox = new OperatorInboxController({ origin: cloud.baseUrl, store: new MemoryInboxStore(), pageOrigin: PAGE_ORIGIN, fetch: async (input, init) => {
      const response = await transport(input, init);
      if (hold && String(input).endsWith("/machine-presence")) {
        hold = false;
        started();
        await gate;
      }
      return response;
    } });
    await signIn(cloud, inbox, ["feed:read", "account:manage"]);
    await inbox.refreshPresence();
    hold = true;
    const earlier = inbox.refreshPresence(true);
    await requested;
    const changed = inbox.setMonitoring(machine.id, false);
    await expect.poll(() => cloud.presence.get(machine.id)?.monitoring).toBe("disabled");
    release();
    await Promise.all([earlier, changed]);
    expect((await inbox.snapshot()).presence?.machines[0].monitoring).toBe("disabled");
  });

  it("refreshes machine status even when feed replay is unavailable", async () => {
    const cloud = new OperatorCloudDouble();
    const machine = await cloud.enrollMachine("hub", [], await exportPublicKey((await generateKeyPair()).publicKey));
    cloud.presence.set(machine.id, { monitoring: "enabled", monitoring_generation: "1", presence: "unobserved" });
    const transport = cloud.fetchFor(PAGE_ORIGIN);
    const inbox = new OperatorInboxController({ origin: cloud.baseUrl, store: new MemoryInboxStore(), pageOrigin: PAGE_ORIGIN, fetch: (input, init) => {
      if (new URL(String(input)).pathname === "/api/operator/feed") throw new TypeError("feed unavailable");
      return transport(input, init);
    } });
    await signIn(cloud, inbox);
    await inbox.runOnce();
    expect((await inbox.snapshot()).presence?.machines[0].presence).toBe("unobserved");
  });

  it("keeps a presence-revoked device signed out after its replay round", async () => {
    const cloud = new OperatorCloudDouble();
    const transport = cloud.fetchFor(PAGE_ORIGIN);
    const inbox = new OperatorInboxController({ origin: cloud.baseUrl, store: new MemoryInboxStore(), pageOrigin: PAGE_ORIGIN, fetch: (input, init) => {
      if (String(input).endsWith("/machine-presence")) return Promise.resolve(new Response(JSON.stringify({ error: "grant_revoked" }), { status: 401 }));
      return transport(input, init);
    } });
    await signIn(cloud, inbox);
    await inbox.runOnce();
    expect(inbox.current().kind).toBe("revoked");
    expect(inbox.client.credential).toBeNull();
  });

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

const target = { hubId: "hub", projectId: "project", sessionId: `s_${createHash("sha256").update("amber-fox-29").digest("base64url")}` };
async function replySetup() {
  const cloud = new OperatorCloudDouble();
  cloud.replyGrants.enabled = true;
  const key = await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"]);
  await cloud.enrollMachine("hub", ["project"], new Uint8Array(await crypto.subtle.exportKey("raw", key.publicKey)), "soundwave");
  const enroll = async (name: string, owner = false) => {
    const store = new MemoryInboxStore();
    const inbox = controller(cloud, store);
    await signIn(cloud, inbox, owner ? ["feed:read", "account:manage"] : undefined, name);
    return { inbox, store };
  };
  return { cloud, enroll };
}

describe("Cloud reply grant upgrades (proposal #174)", () => {



  it("owner approves one request, reader can queue a reply without enrolling again", async () => {
    const { cloud, enroll } = await replySetup();
    const reader = await enroll("reader"); const owner = await enroll("owner", true);
    await reader.inbox.runOnce(); await owner.inbox.runOnce();
    await cloud.appendSessionEvent({ ...target, plaintext: { message: "retained" } });
    await reader.inbox.runOnce();
    const before = reader.store.identity!;
    const history = await reader.store.events(before.accountId, before.feedGeneration);
    const id = before.deviceId;
    const request = await reader.inbox.askToReply(target);
    // A read-only caller cannot use the owner decision route.
    await expect(reader.inbox.client.request({ method: "POST", path: `/api/operator/reply-grant-requests/${request.request_id}/decision`, auth: "grant", body: { wire_version: 1, decision_id: crypto.randomUUID(), decision: "approve", expected_grant_generation: "1" } })).rejects.toMatchObject({ status: 403 });
    await owner.inbox.runOnce();
    expect((await owner.inbox.snapshot()).replyGrants!.requests).toHaveLength(1);
    await owner.inbox.decideReplyRequest(request.request_id, "approve");
    await reader.inbox.runOnce();
    const command = await reader.inbox.queueMessage({ ...target, machineId: [...cloud.machines.keys()][0], sessionName: "amber-fox-29" }, "Now I can reply");
    expect(command.state).toBe("pending_machine");
    expect(reader.store.identity!.deviceId).toBe(id);
    expect(reader.store.identity!.signingKey).toBe(before.signingKey);
    expect(reader.store.identity!.encryption).toBe(before.encryption);
    expect(await reader.store.events(before.accountId, before.feedGeneration)).toEqual(history);
    expect(cloud.devices.size).toBe(2);
  });



  it("retries lost create and approval responses with identical bodies and no second upgrade", async () => {
    const { cloud, enroll } = await replySetup();
    const reader = await enroll("reader"); const owner = await enroll("owner", true);
    await reader.inbox.runOnce(); await owner.inbox.runOnce();
    for (const inbox of [reader.inbox, owner.inbox]) {
      const original = inbox.client.request.bind(inbox.client); let dropped = false;
      vi.spyOn(inbox.client, "request").mockImplementation(async options => {
        const response = await original(options);
        if (!dropped && options.method === "POST" && options.path.includes("reply-grant-requests")) {
          dropped = true; throw new OperatorWireError(0, "network_unavailable", {}, null);
        }
        return response;
      });
    }
    await expect(reader.inbox.askToReply(target)).rejects.toMatchObject({ code: "network_unavailable" });
    const request = await reader.inbox.askToReply(target);
    expect(cloud.replyGrants.requests.size).toBe(1);
    await owner.inbox.runOnce();
    await expect(owner.inbox.decideReplyRequest(request.request_id, "approve")).rejects.toMatchObject({ code: "network_unavailable" });
    await owner.inbox.decideReplyRequest(request.request_id, "approve");
    expect(cloud.devices.get(reader.store.identity!.deviceId)!.generation).toBe(2n);
  });

  it("refuses an unrelated concurrent grant change without removing permissions", async () => {
    const { cloud, enroll } = await replySetup();
    const reader = await enroll("reader"); const owner = await enroll("owner", true);
    await reader.inbox.runOnce(); await owner.inbox.runOnce();
    const request = await reader.inbox.askToReply(target);
    await owner.inbox.runOnce();
    const device = cloud.devices.get(reader.store.identity!.deviceId)!;
    device.generation++; device.capabilities.push("account:manage");
    await expect(owner.inbox.decideReplyRequest(request.request_id, "approve")).rejects.toMatchObject({ code: "reply_grant_generation_conflict" });
    expect(device.capabilities).toContain("account:manage");
    expect(device.scopes).toEqual([]);
    expect(cloud.replyGrants.requests.get(request.request_id)!.status).toBe("pending");
  });

  it("does not adopt a new grant when durable storage fails", async () => {
    const { cloud, enroll } = await replySetup(); const { inbox, store } = await enroll("reader");
    await inbox.runOnce();
    const device = cloud.devices.get(store.identity!.deviceId)!;
    device.generation++;
    device.scopes.push({ hub_id: "hub", project_id: "project", session_id: target.sessionId, operations: ["operator_message"] });
    const save = vi.spyOn(store, "saveIdentity").mockRejectedValueOnce(new Error("quota"));
    await inbox.runOnce();
    expect(inbox.commandScopes()).toEqual([]);
    expect(inbox.client.credential!.generation).toBe("1");
    save.mockRestore(); await inbox.runOnce();
    expect(inbox.commandScopes()).toEqual(device.scopes);
  });

  it("never resurrects an identity from an introspection response after sign-out", async () => {
    const { enroll } = await replySetup(); const { inbox, store } = await enroll("reader");
    const original = inbox.client.request.bind(inbox.client);
    let release!: () => void; let entered!: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    const started = new Promise<void>(resolve => { entered = resolve; });
    vi.spyOn(inbox.client, "request").mockImplementation(async options => {
      const response = await original(options);
      if (options.path === "/api/operator/grants/me") { entered(); await held; }
      return response;
    });
    const refresh = inbox.refreshReplyGrants(); await started; await inbox.signOut(); release(); await refresh;
    expect(store.identity).toBeNull(); expect(inbox.client.credential).toBeNull();
    expect(inbox.current().kind).toBe("signed_out");
  });

});
