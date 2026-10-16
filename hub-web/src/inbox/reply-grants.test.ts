import { createHash } from "node:crypto";
import { describe, expect, it, vi } from "vitest";
import { OperatorCloudDouble } from "../../test/operator-cloud-double";
import { OperatorInboxController } from "./controller";
import { MemoryInboxStore } from "./store";
import { OperatorWireError } from "./wire";
import { ReplyGrantRequests } from "./reply-grants";

const origin = "https://hub.petrastella.io";
const target = { hubId: "hub", projectId: "project", sessionId: `s_${createHash("sha256").update("amber-fox-29").digest("base64url")}` };
async function setup(options: { enabled?: boolean; now?: () => number } = {}) {
  const cloud = new OperatorCloudDouble({ now: options.now });
  cloud.replyGrants.enabled = options.enabled ?? true;
  const key = await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"]);
  await cloud.enrollMachine("hub", ["project"], new Uint8Array(await crypto.subtle.exportKey("raw", key.publicKey)), "soundwave");
  const enroll = async (name: string, owner = false) => {
    const store = new MemoryInboxStore();
    const inbox = new OperatorInboxController({ origin: cloud.baseUrl, pageOrigin: origin, fetch: cloud.fetchFor(origin), store, now: options.now });
    await inbox.load();
    const state = await inbox.beginSignIn(name);
    if (state.kind !== "awaiting_approval") throw new Error("missing code");
    await cloud.approve(state.userCode, { capabilities: ["feed:read", ...(owner ? ["account:manage"] : [])] });
    await inbox.pollSignIn();
    return { inbox, store };
  };
  return { cloud, enroll };
}

describe("Cloud reply grant upgrades (proposal #174)", () => {
  it("refreshes a stale grant in place before replay, preserving identity keys and history", async () => {
    const { cloud, enroll } = await setup();
    const { inbox, store } = await enroll("reader");
    await cloud.appendSessionEvent({ ...target, plaintext: { message: "retained" } });
    await inbox.runOnce();
    const before = store.identity!;
    const events = await store.events(before.accountId, before.feedGeneration);
    const device = cloud.devices.get(before.deviceId)!;
    device.scopes.push({ hub_id: "hub", project_id: "project", session_id: target.sessionId, operations: ["operator_message"] });
    device.generation++;
    await inbox.runOnce();
    expect(inbox.commandScopes()).toEqual(device.scopes);
    expect(store.identity!.grant.generation).toBe("2");
    expect(store.identity!.deviceId).toBe(before.deviceId);
    expect(store.identity!.signingKey).toBe(before.signingKey);
    expect(store.identity!.encryption).toBe(before.encryption);
    expect(await store.events(before.accountId, before.feedGeneration)).toEqual(events);
  });

  it("keeps the unshipped feature hidden and never calls proposed routes", async () => {
    const { cloud, enroll } = await setup({ enabled: false });
    const { inbox } = await enroll("reader");
    await inbox.runOnce();
    expect((await inbox.snapshot()).replyGrants?.supported).toBe(false);
    expect(cloud.log.some(r => r.path.includes("reply-grant-requests"))).toBe(false);
    await expect(inbox.askToReply(target)).rejects.toThrow(/not available/);
  });

  it("owner approves one request, reader can queue a reply without enrolling again", async () => {
    const { cloud, enroll } = await setup();
    const reader = await enroll("reader"); const owner = await enroll("owner", true);
    await reader.inbox.runOnce(); await owner.inbox.runOnce();
    const id = reader.store.identity!.deviceId;
    const request = await reader.inbox.askToReply(target);
    await owner.inbox.runOnce();
    expect((await owner.inbox.snapshot()).replyGrants!.requests).toHaveLength(1);
    await owner.inbox.decideReplyRequest(request.request_id, "approve");
    await reader.inbox.runOnce();
    const command = await reader.inbox.queueMessage({ ...target, machineId: [...cloud.machines.keys()][0], sessionName: "amber-fox-29" }, "Now I can reply");
    expect(command.state).toBe("pending_machine");
    expect(reader.store.identity!.deviceId).toBe(id);
    expect(cloud.devices.size).toBe(2);
  });

  it("denial and expiry remain read-only, and a reader cannot approve itself", async () => {
    let now = Date.now();
    const { cloud, enroll } = await setup({ now: () => now });
    const reader = await enroll("reader"); const owner = await enroll("owner", true);
    await reader.inbox.runOnce(); await owner.inbox.runOnce();
    let request = await reader.inbox.askToReply(target);
    const ownService = new ReplyGrantRequests(reader.inbox.client); ownService.configure(["reply-grant-upgrade-v1"], true);
    await expect(ownService.decide(request, "approve")).rejects.toMatchObject({ status: 403 });
    await owner.inbox.runOnce(); await owner.inbox.decideReplyRequest(request.request_id, "deny"); await reader.inbox.runOnce();
    expect((await reader.inbox.snapshot()).replyGrants!.requests[0].status).toBe("denied");
    request = await reader.inbox.askToReply(target); now += 600_001;
    // Signatures use the same server/test clock; no real clock sleeps.
    await reader.inbox.runOnce();
    expect((await reader.inbox.snapshot()).replyGrants!.requests.find(r => r.request_id === request.request_id)?.status).toBe("expired");
    expect(reader.inbox.commandScopes()).toEqual([]);
    expect(cloud.devices.get(reader.store.identity!.deviceId)!.generation).toBe(1n);
  });

  it("request and decision retries preserve IDs and grant generation; conflicting bodies refuse", async () => {
    const { cloud, enroll } = await setup();
    const reader = await enroll("reader"); const owner = await enroll("owner", true);
    await reader.inbox.runOnce(); await owner.inbox.runOnce();
    const body = { wire_version: 1, request_id: crypto.randomUUID(), hub_id: "hub", project_id: "project", session_id: target.sessionId, expected_grant_generation: "1" };
    const create = (input = body) => reader.inbox.client.request({ method: "POST", path: "/api/operator/reply-grant-requests", auth: "grant", body: input });
    expect(await create()).toEqual(await create());
    await expect(create({ ...body, session_id: "other" })).rejects.toMatchObject({ code: "reply_request_conflict" });
    const decide = { wire_version: 1, decision_id: crypto.randomUUID(), decision: "approve", expected_grant_generation: "1" };
    const post = (input = decide) => owner.inbox.client.request({ method: "POST", path: `/api/operator/reply-grant-requests/${body.request_id}/decision`, auth: "grant", body: input });
    expect(await post()).toEqual(await post());
    expect(cloud.devices.get(reader.store.identity!.deviceId)!.generation).toBe(2n);
    await expect(post({ ...decide, decision: "deny" })).rejects.toMatchObject({ code: "reply_decision_conflict" });
  });
  it("retries lost create and approval responses with identical bodies and no second upgrade", async () => {
    const { cloud, enroll } = await setup();
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
    const { cloud, enroll } = await setup();
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
    const { cloud, enroll } = await setup(); const { inbox, store } = await enroll("reader");
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
    const { enroll } = await setup(); const { inbox, store } = await enroll("reader");
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
