import { readFile } from "node:fs/promises";
import { describe, expect, it, vi } from "vitest";
import { consumePairingFragment } from "./fragment";
import { exchangePendingPairing, PairingExchangeError } from "./pairing-exchange";
import { PendingPairingStore } from "./pending-pairing";
import {
  DEFAULT_PAIRING_SCOPES,
  PairingRelayError,
  acknowledgePairing,
  createPairingRequest,
  pollPairingRequest,
} from "./pairing-relay";
import type { StoredMachine } from "./types";

class MemoryStorage implements Storage {
  readonly values = new Map<string, string>();
  get length(): number { return this.values.size; }
  clear(): void { this.values.clear(); }
  getItem(key: string): string | null { return this.values.get(key) ?? null; }
  key(index: number): string | null { return [...this.values.keys()][index] ?? null; }
  removeItem(key: string): void { this.values.delete(key); }
  setItem(key: string, value: string): void { this.values.set(key, value); }
}

const request = {
  kind: "relay-request" as const,
  pairingRequestId: "9ef5a981-0c32-44b4-9c8a-d1f8e4858e77",
  userCode: "K7MW-4H2Q",
  pollSecret: "poll-secret-do-not-display",
  controllerOrigin: "https://commander.example",
  requestedScopes: DEFAULT_PAIRING_SCOPES,
  expiresAt: "2026-08-11T20:10:00Z",
  interval: 3,
};

function response(status: number, body?: unknown, headers?: Record<string, string>): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { ...(body === undefined ? {} : { "Content-Type": "application/json" }), ...headers },
  });
}

describe("wire-v1 reverse pairing", () => {
  it("parses all six section-4 fixtures copied into CAS", async () => {
    const names = [
      "create-request", "claim-request", "complete-request", "pending-poll-response",
      "ready-poll-response", "acknowledge-request",
    ];
    const fixtures = await Promise.all(names.map(async (name) => JSON.parse(await readFile(new URL(`fixtures/hub-reverse-pairing/${name}.json`, import.meta.url), "utf8"))));
    expect(fixtures.map((fixture) => fixture.wire_version)).toEqual([1, 1, 1, 1, 1, 1]);
    expect(fixtures[0].requested_scopes).toEqual(["machine-read", "session-read", "pane-read"]);
    expect(fixtures[4].invitation.scopes).toEqual(["machine-read", "session-read", "pane-read"]);
  });

  it("creates with exact origin/default scopes and credentials omitted", async () => {
    const fetcher = vi.fn(async () => response(201, {
      wire_version: 1,
      pairing_request_id: request.pairingRequestId,
      user_code: request.userCode,
      poll_secret: request.pollSecret,
      controller_origin: request.controllerOrigin,
      requested_scopes: request.requestedScopes,
      expires_at: request.expiresAt,
      expires_in: 600,
      interval: request.interval,
      email_sent: false,
    }));
    await expect(createPairingRequest(fetcher, request.controllerOrigin)).resolves.toEqual(request);
    expect(fetcher).toHaveBeenCalledWith("/api/hub/pairing/requests", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "omit",
      body: JSON.stringify({ wire_version: 1, controller_origin: request.controllerOrigin, requested_scopes: request.requestedScopes }),
    });
  });

  it.each(["https://commander.example:444", "https://evil.example"])("rejects returned origin %s without exposing the secret", async (returnedOrigin) => {
    const fetcher = vi.fn(async () => response(201, {
      wire_version: 1, pairing_request_id: request.pairingRequestId, user_code: request.userCode,
      poll_secret: request.pollSecret, controller_origin: returnedOrigin,
      requested_scopes: request.requestedScopes, expires_at: request.expiresAt, expires_in: 600, interval: 3, email_sent: false,
    }));
    const error = await createPairingRequest(fetcher, request.controllerOrigin).catch((caught) => caught);
    expect(error).toBeInstanceOf(PairingRelayError);
    expect(String(error)).not.toContain(request.pollSecret);
  });

  it.each(["BAD-CODE", "K7MW4H2Q", "K7MW-4H2I"])("rejects malformed user code %s", async (userCode) => {
    const fetcher = vi.fn(async () => response(201, {
      wire_version: 1, pairing_request_id: request.pairingRequestId, user_code: userCode,
      poll_secret: request.pollSecret, controller_origin: request.controllerOrigin,
      requested_scopes: request.requestedScopes, expires_at: request.expiresAt, expires_in: 600, interval: 3, email_sent: false,
    }));
    await expect(createPairingRequest(fetcher, request.controllerOrigin)).rejects.toThrow("invalid pairing code");
  });

  it("handles pending, claimed, slow-down, denial, and expiry without leaking relay secrets", async () => {
    const pending = vi.fn(async () => response(202, { wire_version: 1, status: "authorization_pending", expires_at: request.expiresAt, interval: 3 }));
    await expect(pollPairingRequest(pending, request)).resolves.toEqual({ kind: "pending", interval: 3, expiresAt: request.expiresAt });
    const claimed = vi.fn(async () => response(202, { wire_version: 1, status: "machine_claimed", expires_at: request.expiresAt, interval: 3 }));
    await expect(pollPairingRequest(claimed, request)).resolves.toEqual({ kind: "claimed", interval: 3, expiresAt: request.expiresAt });
    const slow = vi.fn(async () => response(429, { error: "slow_down", error_description: "wait" }, { "Retry-After": "7" }));
    await expect(pollPairingRequest(slow, request)).resolves.toEqual({ kind: "slow-down", interval: 7 });
    for (const [status, code] of [[403, "request_mismatch"], [410, "expired_request"]] as const) {
      const fetcher = vi.fn(async () => response(status, { error: code, error_description: request.pollSecret }));
      const error = await pollPairingRequest(fetcher, request).catch((caught) => caught);
      expect(error).toBeInstanceOf(PairingRelayError);
      expect(String(error)).not.toContain(request.pollSecret);
    }
  });

  it("rejects an authorized payload for the wrong origin, port, or unsafe hub URL", async () => {
    const base = {
      wire_version: 1, status: "authorized", delivery_id: "75128f2d-845b-4d2b-9d42-ffdb74661ca2",
      invitation: { token: "one-time-token", hub_id: "machine-uuid", hub_url: "https://workstation.tail.example", machine_label: "Studio workstation", controller_origin: request.controllerOrigin, scopes: request.requestedScopes, expires_at: "2026-08-11T20:11:30Z" },
    };
    for (const invitation of [
      { ...base.invitation, controller_origin: `${request.controllerOrigin}:444` },
      { ...base.invitation, controller_origin: "https://evil.example" },
      { ...base.invitation, hub_url: "http://workstation.example" },
      { ...base.invitation, hub_url: "https://workstation.tail.example/path" },
    ]) {
      const fetcher = vi.fn(async () => response(200, { ...base, invitation }));
      await expect(pollPairingRequest(fetcher, request)).rejects.toBeInstanceOf(PairingRelayError);
    }
  });

  it("restores a pending request after refresh and removes it at authoritative expiry", () => {
    const storage = new MemoryStorage();
    const firstPage = new PendingPairingStore(storage, () => Date.parse("2026-08-11T20:09:00Z"));
    firstPage.save(request);
    const refreshedPage = new PendingPairingStore(storage, () => Date.parse("2026-08-11T20:09:30Z"));
    expect(refreshedPage.load()).toEqual(request);
    const expiredPage = new PendingPairingStore(storage, () => Date.parse("2026-08-11T20:10:00Z"));
    expect(expiredPage.load()).toBeNull();
    expect(storage.length).toBe(0);
  });

  it("persists a legacy fragment before scrubbing it and restores it after refresh", () => {
    const token = "A".repeat(43);
    const storage = new MemoryStorage();
    const store = new PendingPairingStore(storage);
    let replacement = "";
    const location = { hash: `#pair=${token}&hub=machine-uuid`, pathname: "/", search: "" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;
    expect(consumePairingFragment(location, history, store)).toMatchObject({ kind: "invitation", token, hubId: "machine-uuid" });
    expect(replacement).toBe("/");
    expect(replacement).not.toContain(token);
    expect(new PendingPairingStore(storage).load()).toMatchObject({ token, hubId: "machine-uuid" });
  });

  it("scrubs a malformed legacy capability even when it cannot be persisted", () => {
    let replacement = "";
    const location = { hash: "#pair=malformed-secret&hub=machine-uuid", pathname: "/commander", search: "?view=fleet" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;
    expect(consumePairingFragment(location, history)).toBeNull();
    expect(replacement).toBe("/commander?view=fleet");
  });

  it("persists before acknowledgement and keeps local success when acknowledgement fails", async () => {
    const events: string[] = [];
    const fetcher = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      if (String(input).includes("acknowledge")) { events.push("ack"); throw new Error("relay unavailable"); }
      return response(200, { device_id: "device", credential_id: "credential", credential: "opaque", expires_at: "2027-01-01T00:00:00Z", scopes: ["machine-read"] });
    });
    const invitation = {
      kind: "invitation" as const,
      token: "one-time-token",
      hubId: "machine-uuid",
      hubUrl: "https://workstation.tail.example",
      machineLabel: "Studio workstation",
      controllerOrigin: request.controllerOrigin,
      scopes: ["machine-read"] as const,
      expiresAt: "2026-08-11T20:11:30Z",
      relay: { pairingRequestId: request.pairingRequestId, pollSecret: request.pollSecret, deliveryId: "75128f2d-845b-4d2b-9d42-ffdb74661ca2" },
    };
    const persist = vi.fn(async (_machine: StoredMachine) => { events.push("persist"); });
    await expect(exchangePendingPairing({
      invitation, controllerOrigin: request.controllerOrigin, deviceLabel: "Browser", operatorLabel: "Operator",
      fetcher, createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }), persist,
      acknowledge: (relay) => acknowledgePairing(fetcher, relay),
    })).resolves.toMatchObject({ id: invitation.hubId, baseUrl: invitation.hubUrl });
    expect(events).toEqual(["persist", "ack"]);
    const [hubTarget, hubInit] = fetcher.mock.calls[0];
    expect(String(hubTarget)).toBe("https://workstation.tail.example/v1/auth/pairing/exchange");
    expect(JSON.parse(String(hubInit?.body))).toMatchObject({ controller_origin: request.controllerOrigin, requested_scopes: ["machine-read"] });
  });

  it("lets the hub's one-time exchange choose one winner across two tabs", async () => {
    let exchanged = false;
    const fetcher = vi.fn(async () => {
      if (exchanged) return response(401, { error: "unauthorized" });
      exchanged = true;
      return response(200, { device_id: "device", credential_id: "credential", credential: "opaque", expires_at: "2027-01-01T00:00:00Z", scopes: ["machine-read"] });
    });
    const invitation = { kind: "invitation" as const, token: "one-time-token", hubId: "machine-uuid", hubUrl: "https://workstation.tail.example", machineLabel: "Studio workstation", controllerOrigin: request.controllerOrigin, scopes: ["machine-read"] as const, expiresAt: "2026-08-11T20:11:30Z" };
    const persisted: StoredMachine[] = [];
    const run = () => exchangePendingPairing({
      invitation, controllerOrigin: request.controllerOrigin, deviceLabel: "Browser", operatorLabel: "Operator",
      fetcher, createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }), persist: async (machine) => { persisted.push(machine); },
    });
    const settled = await Promise.allSettled([run(), run()]);
    expect(settled.map((result) => result.status).sort()).toEqual(["fulfilled", "rejected"]);
    expect((settled.find((result) => result.status === "rejected") as PromiseRejectedResult).reason).toBeInstanceOf(PairingExchangeError);
    expect(persisted).toHaveLength(1);
  });
});
