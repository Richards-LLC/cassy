// cas-9b7d S1/S3: enrollment, epoch keys and replay against the protocol
// double of the cloud contract (test/operator-cloud-double.ts). Real
// IndexedDB is exercised by journeys; these tests use the same store contract
// in memory.

import { describe, expect, it } from "vitest";
import { OperatorCloudDouble } from "../../test/operator-cloud-double";
import { completeEnrollment, pollEnrollment, refreshEpochKeys, startEnrollment } from "./enrollment";
import { exportPublicKey, generateKeyPair } from "./hpke";
import { IssuerKeys } from "./issuer";
import { flushAcks, replayRound, verifyCoverage, ReplayPageError, type ReplayContext } from "./replay";
import { MemoryInboxStore, type InboxIdentity } from "./store";
import { OperatorClient, OperatorWireError } from "./wire";

const PAGE_ORIGIN = "https://hub.petrastella.io";

interface Device {
  client: OperatorClient;
  store: MemoryInboxStore;
  issuer: IssuerKeys;
  identity: InboxIdentity;
  context: ReplayContext;
}

function clientFor(cloud: OperatorCloudDouble, pageOrigin = PAGE_ORIGIN, now?: () => number) {
  return new OperatorClient({ origin: cloud.baseUrl, fetch: cloud.fetchFor(pageOrigin), now });
}

async function enroll(cloud: OperatorCloudDouble, label: string, approve: Parameters<OperatorCloudDouble["approve"]>[1] = {}): Promise<Device> {
  const client = clientFor(cloud);
  const store = new MemoryInboxStore();
  const issuer = new IssuerKeys(() => cloud.jwks());
  const pending = await startEnrollment(client, store, { pageOrigin: PAGE_ORIGIN, label });
  expect(await pollEnrollment(client, pending)).toEqual({ status: "pending", intervalS: 5 });
  await cloud.approve(pending.userCode, approve);
  const outcome = await pollEnrollment(client, pending);
  expect(outcome.status).toBe("approved");
  const identity = await completeEnrollment(client, store, issuer, pending, outcome.status === "approved" ? outcome.emailHint : null);
  const context: ReplayContext = {
    client,
    store,
    issuer,
    identity,
    machines: async () => {
      const principals = await client.principals();
      const ids = new Set((principals.machines as { machine_id: string }[]).map((machine) => machine.machine_id));
      return { has: (id: string) => ids.has(id) };
    },
    refreshKeys: async () => {
      await refreshEpochKeys(client, store, issuer, identity);
    },
  };
  return { client, store, issuer, identity, context };
}

const turn = (body: string) => ({ type: "cas.operator.turn", v: 1, role: "supervisor", body });

async function machine(cloud: OperatorCloudDouble) {
  return cloud.enrollMachine("hub-soundwave", ["proj-cas"], await exportPublicKey((await generateKeyPair()).publicKey));
}

describe("enrollment (§5.1–§5.3)", () => {
  it("enrolls under account permission alone and receives every retained epoch key", async () => {
    const cloud = new OperatorCloudDouble({ baseUrl: "https://psc-double.test" });
    const device = await enroll(cloud, "Pixel 9 — Chrome");
    expect(device.identity.accountId).toBe(cloud.accountId);
    expect(device.identity.grant.capabilities).toContain("feed:read");
    expect(device.store.pending).toBeNull();
    expect(await device.store.epochKey(cloud.accountId, "1", "1")).not.toBeNull();
    expect(device.identity.signingKey.privateKey.extractable).toBe(false);
    expect(device.identity.encryption.privateKey.extractable).toBe(false);
  });

  it("is refused for an origin the cloud does not allow", async () => {
    const cloud = new OperatorCloudDouble();
    const client = clientFor(cloud, "https://evil.example");
    await expect(startEnrollment(client, new MemoryInboxStore(), { pageOrigin: "https://evil.example", label: "x" })).rejects.toMatchObject({
      code: "origin_mismatch",
    });
  });

  it("reports a denied or expired challenge without minting a grant", async () => {
    const cloud = new OperatorCloudDouble();
    const client = clientFor(cloud);
    const store = new MemoryInboxStore();
    const pending = await startEnrollment(client, store, { pageOrigin: PAGE_ORIGIN, label: "denied" });
    await cloud.approve(pending.userCode, { deny: true });
    expect(await pollEnrollment(client, pending)).toEqual({ status: "denied" });
    await expect(
      completeEnrollment(client, store, new IssuerKeys(() => cloud.jwks()), pending, null),
    ).rejects.toMatchObject({ code: "enrollment_not_approved" });
    expect(cloud.devices.size).toBe(0);
  });

  it("re-signs once with the server clock when the device clock is wrong", async () => {
    const cloud = new OperatorCloudDouble();
    const device = await enroll(cloud, "skewed");
    const skewed = new OperatorClient({ origin: cloud.baseUrl, fetch: cloud.fetchFor(PAGE_ORIGIN), now: () => Date.now() + 10 * 60_000 });
    skewed.credential = device.client.credential;
    const grant = await skewed.grantsMe();
    expect((grant.grant as { grant_id: string }).grant_id).toBe(device.identity.deviceId);
    expect(cloud.log.filter((entry) => entry.code === "pop_expired")).toHaveLength(1);
  });

  it("never accepts a replayed proof", async () => {
    const cloud = new OperatorCloudDouble();
    const device = await enroll(cloud, "replay");
    let captured: { url: string; init: RequestInit } | null = null;
    const recorder = new OperatorClient({
      origin: cloud.baseUrl,
      fetch: async (input, init) => {
        captured = { url: String(input), init: init! };
        return cloud.fetchFor(PAGE_ORIGIN)(input, init);
      },
    });
    recorder.credential = device.client.credential;
    await recorder.grantsMe();
    const replayed = await cloud.fetchFor(PAGE_ORIGIN)(captured!.url, captured!.init);
    expect(replayed.status).toBe(401);
    expect((await replayed.json()).error).toBe("pop_replay");
  });
});

describe("replay (§8.1–§8.3)", () => {
  it("lets two devices replay and persist the same history independently", async () => {
    const cloud = new OperatorCloudDouble();
    const phone = await enroll(cloud, "phone");
    await machine(cloud);
    for (const body of ["one", "two", "three"]) {
      await cloud.appendSessionEvent({ hubId: "hub-soundwave", projectId: "proj-cas", sessionId: "s_alpha", plaintext: turn(body) });
    }
    const first = await replayRound(phone.context);
    expect(first.kind).toBe("caught_up");
    const phoneEvents = await phone.store.events(cloud.accountId, "1");
    expect(phoneEvents.map((event) => (event.plaintext as { body: string }).body)).toEqual(["one", "two", "three"]);
    expect(phoneEvents.every((event) => event.verification === "verified" && event.acked)).toBe(true);
    expect(cloud.acks.get(phone.identity.deviceId)?.size).toBe(3);

    // A desktop enrolled later (the phone is now offline) gets everything too;
    // the phone's ACKs and cursor do not suppress its replay.
    const desktop = await enroll(cloud, "desktop");
    await replayRound(desktop.context);
    expect((await desktop.store.events(cloud.accountId, "1")).map((event) => event.eventId)).toEqual(phoneEvents.map((event) => event.eventId));
    expect(cloud.acks.get(desktop.identity.deviceId)?.size).toBe(3);
    expect(cloud.cursors.get(`${desktop.identity.deviceId}|1`)?.cursor).toBe(3n);
  });

  it("pages until caught up and stores each event exactly once", async () => {
    const cloud = new OperatorCloudDouble();
    const device = await enroll(cloud, "pager");
    cloud.pageLimit = 2;
    for (let index = 0; index < 5; index += 1) {
      await cloud.appendSessionEvent({ hubId: "h1", projectId: "p1", sessionId: "s1", plaintext: turn(`m${index}`) });
    }
    const outcome = await replayRound(device.context);
    expect(outcome).toMatchObject({ kind: "caught_up", pages: 3 });
    const again = await replayRound(device.context);
    expect(again).toMatchObject({ kind: "caught_up", stored: [] });
    expect(await device.store.events(cloud.accountId, "1")).toHaveLength(5);
  });

  it("records an expired gap as accepted history loss, never as an empty success", async () => {
    const cloud = new OperatorCloudDouble();
    for (let index = 0; index < 4; index += 1) {
      await cloud.appendSessionEvent({ hubId: "h1", projectId: "p1", sessionId: "s1", plaintext: turn(`m${index}`) });
    }
    cloud.expireThrough(2n);
    const device = await enroll(cloud, "late");
    await replayRound(device.context);
    const cursor = await device.store.loadCursor(cloud.accountId, "1");
    expect(cursor).toMatchObject({ cursor: "4", acceptedExpiredThrough: "2" });
    expect(cursor?.expired).toEqual([{ from: "1", to: "2", reason: "retention" }]);
    expect((await device.store.events(cloud.accountId, "1")).map((event) => event.sequence)).toEqual(["3", "4"]);
    expect(cloud.log.some((entry) => entry.code === "history_expired")).toBe(true);
  });

  it("keeps the cursor and sends no ACK when the local commit fails", async () => {
    const cloud = new OperatorCloudDouble();
    const device = await enroll(cloud, "full-disk");
    await cloud.appendSessionEvent({ hubId: "h1", projectId: "p1", sessionId: "s1", plaintext: turn("kept") });
    device.store.failNextCommit = new DOMException("Quota exceeded", "QuotaExceededError");
    await expect(replayRound(device.context)).rejects.toThrow("Quota exceeded");
    expect(await device.store.loadCursor(cloud.accountId, "1")).toBeNull();
    expect(cloud.acks.get(device.identity.deviceId)?.size ?? 0).toBe(0);
    await replayRound(device.context);
    expect(cloud.acks.get(device.identity.deviceId)?.size).toBe(1);
  });

  it("re-sends ACKs left unacknowledged by a crash after commit", async () => {
    const cloud = new OperatorCloudDouble();
    const device = await enroll(cloud, "crash");
    await cloud.appendSessionEvent({ hubId: "h1", projectId: "p1", sessionId: "s1", plaintext: turn("x") });
    cloud.faults.push({ method: "POST", pathPrefix: "/api/operator/feed/acks", status: 503, code: "temporarily_unavailable", times: 1 });
    await expect(replayRound(device.context)).rejects.toBeInstanceOf(OperatorWireError);
    expect(await device.store.loadCursor(cloud.accountId, "1")).toMatchObject({ cursor: "1" });
    expect(await device.store.unacked(cloud.accountId, "1")).toHaveLength(1);
    expect(await flushAcks(device.context)).toBe(1);
    expect(await device.store.unacked(cloud.accountId, "1")).toHaveLength(0);
  });

  it("verifies observer notices before persisting them as notices (§7.4)", async () => {
    const cloud = new OperatorCloudDouble();
    const device = await enroll(cloud, "observer");
    const soundwave = await machine(cloud);
    await cloud.appendObserverNotice(soundwave, "machine_unobserved");
    await cloud.appendObserverNotice(soundwave, "machine_unobserved", { tamperClaim: "outage_epoch" });
    await replayRound(device.context);
    const [good, bad] = await device.store.events(cloud.accountId, "1");
    expect(good).toMatchObject({ scope: "machine", verification: "verified", acked: true, projectId: null });
    expect((good.plaintext as { kind: string }).kind).toBe("machine_unobserved");
    expect(bad).toMatchObject({ verification: "unverified", failure: "plaintext_mismatch", acked: false, plaintext: null });
    // Coverage still moves past the unverified row.
    expect(await device.store.loadCursor(cloud.accountId, "1")).toMatchObject({ cursor: "2" });
    expect(cloud.acks.get(device.identity.deviceId)?.size).toBe(1);
  });

  it("honors revoke and epoch rotation", async () => {
    const cloud = new OperatorCloudDouble();
    const lost = await enroll(cloud, "lost phone");
    const desktop = await enroll(cloud, "desktop");
    await cloud.appendSessionEvent({ hubId: "h1", projectId: "p1", sessionId: "s1", plaintext: turn("before") });
    await cloud.revokeDevice(lost.identity.deviceId);
    await cloud.appendSessionEvent({ hubId: "h1", projectId: "p1", sessionId: "s1", plaintext: turn("after") });

    await expect(replayRound(lost.context)).rejects.toMatchObject({ code: "grant_revoked" });
    await expect(lost.client.keyWraps()).rejects.toMatchObject({ code: "grant_revoked" });

    await replayRound(desktop.context);
    const events = await desktop.store.events(cloud.accountId, "1");
    expect(events.map((event) => [event.keyEpoch, (event.plaintext as { body: string }).body])).toEqual([
      ["1", "before"],
      ["2", "after"],
    ]);
  });

  it("returns a feed generation change to the caller instead of adopting it silently", async () => {
    const cloud = new OperatorCloudDouble();
    const device = await enroll(cloud, "reset");
    await cloud.appendSessionEvent({ hubId: "h1", projectId: "p1", sessionId: "s1", plaintext: turn("old") });
    await replayRound(device.context);
    await cloud.resetFeed();
    expect(await replayRound(device.context)).toEqual({ kind: "generation_changed", feedGeneration: "2", startSequence: "2" });
    await expect(refreshEpochKeys(device.client, device.store, device.issuer, device.identity)).rejects.toMatchObject({ reason: "generation_changed" });
  });
});

describe("coverage (§8.1)", () => {
  const event = (sequence: string) => ({
    sequence,
    eventId: `e${sequence}`,
    scope: "session",
    producerKind: "principal",
    hubId: "h",
    projectId: "p",
    sessionId: "s",
    keyEpoch: "1",
    ciphertext: "",
    digest: "",
    storedAt: "",
    expiresAt: "",
    observerAssertion: null,
  });

  it("accepts exact coverage and refuses gaps, overlaps and overruns", () => {
    expect(() => verifyCoverage("10", "14", [event("11"), event("14")], [{ from: "12", to: "13", reason: "retention" }])).not.toThrow();
    expect(() => verifyCoverage("10", "10", [], [])).not.toThrow();
    expect(() => verifyCoverage("10", "13", [event("11"), event("13")], [])).toThrow(ReplayPageError);
    expect(() => verifyCoverage("10", "12", [event("11"), event("12")], [{ from: "12", to: "12", reason: "retention" }])).toThrow(/overlap/);
    expect(() => verifyCoverage("10", "11", [event("11"), event("12")], [])).toThrow(/beyond/);
    expect(() => verifyCoverage("10", "9", [], [])).toThrow(/below/);
  });
});
