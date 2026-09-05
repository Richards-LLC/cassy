import { readFile } from "node:fs/promises";
import { describe, expect, it, vi } from "vitest";
import { consumePairingFragment, watchPairingFragment } from "./fragment";
import { createPairingDraft, updatePairingDraft } from "./pairing-draft";
import { bindPairingDialogCancel } from "./pairing-dialog";
import { pairingCleanupFailureUpdate, pairingStorageClearFailureMessage } from "./pairing-cleanup";
import { exchangePendingPairing, PairingCleanupError, PairingExchangeError } from "./pairing-exchange";
import { PairingOperationCoordinator, commitPairingResult } from "./pairing-operation";
import { PendingPairingStore, pendingPairingStoreFor } from "./pending-pairing";
import {
  DEFAULT_PAIRING_SCOPES,
  PairingRelayError,
  acknowledgePairing,
  createPairingRequest,
  normalizeHubOrigin,
  pairingRelayOrigin,
  pollPairingRequest,
} from "./pairing-relay";
import { MachineCatalog, type MachineCatalogBackend, type MachineCatalogRecord } from "./storage";
import type { PairingInstallIdentity, StoredMachine } from "./types";

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

async function fixture(name: string): Promise<Record<string, unknown>> {
  return JSON.parse(await readFile(new URL(`fixtures/hub-reverse-pairing/${name}.json`, import.meta.url), "utf8")) as Record<string, unknown>;
}

const relayOrigin = "https://petra-stella-cloud.vercel.app";

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  return { promise: new Promise<T>((done) => { resolve = done; }), resolve };
}

async function exchangeFailure(attempt: Promise<unknown>): Promise<PairingExchangeError> {
  try {
    await attempt;
  } catch (error) {
    return error as PairingExchangeError;
  }
  throw new Error("expected pairing exchange to fail");
}

class MemoryMachineCatalogBackend implements MachineCatalogBackend {
  readonly records = new Map<string, MachineCatalogRecord>();
  rejectUpdate = false;
  rejectDelete = false;
  rejectNextUpdate = false;

  async list(): Promise<MachineCatalogRecord[]> { return [...this.records.values()]; }

  async update(
    id: string,
    change: (current: MachineCatalogRecord | undefined) => MachineCatalogRecord | undefined,
  ): Promise<void> {
    const next = change(this.records.get(id));
    if (this.rejectNextUpdate) {
      this.rejectNextUpdate = false;
      throw new Error("durable cleanup rejected");
    }
    if (this.rejectUpdate || (this.rejectDelete && !next)) throw new Error("durable cleanup rejected");
    if (next) this.records.set(id, next);
    else this.records.delete(id);
  }
}

function storedMachine(credentialId: string, credential = `opaque-${credentialId}`): StoredMachine {
  return {
    id: "machine-uuid", label: credentialId, baseUrl: "https://workstation.tail.example",
    deviceId: `device-${credentialId}`, credentialId, credential,
    expiresAt: "2027-01-01T00:00:00Z", scopes: ["machine-read"],
    publicKey: { kty: "EC" }, privateKey: {} as CryptoKey,
  };
}

function installIdentity(credentialId: string, generation: number): PairingInstallIdentity {
  return { machineId: "machine-uuid", credentialId, generation };
}

describe("wire-v1 reverse pairing", () => {
  it("loads the seven shared section-4 vectors used by production serializers and parsers", async () => {
    const names = [
      "create-request", "create-response", "claim-request", "complete-request", "pending-poll-response",
      "ready-poll-response", "acknowledge-request",
    ];
    const fixtures = await Promise.all(names.map(fixture));
    expect(fixtures.map((value) => value.wire_version)).toEqual([1, 1, 1, 1, 1, 1, 1]);
    expect(fixtures[0].requested_scopes).toEqual(["machine-read", "session-read", "pane-read"]);
    expect((fixtures[5].invitation as Record<string, unknown>).scopes).toEqual(["machine-read", "session-read", "pane-read"]);
  });

  it("requests Cassy Commander control scopes through the relay while continuing to accept narrowed invitations", () => {
    expect(DEFAULT_PAIRING_SCOPES).toEqual([
      "machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt",
    ]);
  });

  it.each([
    ["embedded controller hub", "https://controller.tail.example"],
    ["hosted static controller", "https://hub.petrastella.io"],
  ])("routes %s create traffic to the explicit relay, never the controller origin", async (_mode, controllerOrigin) => {
    const createResponse = { ...await fixture("create-response"), controller_origin: controllerOrigin, requested_scopes: DEFAULT_PAIRING_SCOPES };
    const createRequest = { ...await fixture("create-request"), controller_origin: controllerOrigin, requested_scopes: DEFAULT_PAIRING_SCOPES };
    const fetcher = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => response(201, createResponse));
    await expect(createPairingRequest(fetcher, relayOrigin, controllerOrigin, DEFAULT_PAIRING_SCOPES, "operator@example.com")).resolves.toMatchObject({
      kind: "relay-request", controllerOrigin,
    });
    const [target, init] = fetcher.mock.calls[0];
    expect(String(target)).toBe(`${relayOrigin}/api/hub/pairing/requests`);
    expect(new URL(String(target)).origin).not.toBe(controllerOrigin);
    expect(init).toMatchObject({ method: "POST", credentials: "omit" });
    expect(JSON.parse(String(init?.body))).toEqual(createRequest);
  });

  it("fails closed when relay metadata is missing or is not an HTTPS origin", () => {
    expect(pairingRelayOrigin(null)).toBeNull();
    for (const unsafe of [
      "", "http://relay.example", "https://user:pass@relay.example", "https://relay.example/path",
      "https://relay.example?query=1", "https://relay.example/#fragment",
    ]) expect(pairingRelayOrigin(unsafe)).toBeNull();
    expect(pairingRelayOrigin(`${relayOrigin}/`)).toBe(relayOrigin);
  });

  it.each(["https://commander.example:444", "https://evil.example"])("rejects returned origin %s without exposing the secret", async (returnedOrigin) => {
    const fetcher = vi.fn(async () => response(201, {
      wire_version: 1, pairing_request_id: request.pairingRequestId, user_code: request.userCode,
      poll_secret: request.pollSecret, controller_origin: returnedOrigin,
      requested_scopes: request.requestedScopes, expires_at: request.expiresAt, expires_in: 600, interval: 3, email_sent: false,
    }));
    const error = await createPairingRequest(fetcher, relayOrigin, request.controllerOrigin).catch((caught) => caught);
    expect(error).toBeInstanceOf(PairingRelayError);
    expect(String(error)).not.toContain(request.pollSecret);
  });

  it.each(["BAD-CODE", "K7MW4H2Q", "K7MW-4H2I"])("rejects malformed user code %s", async (userCode) => {
    const fetcher = vi.fn(async () => response(201, {
      wire_version: 1, pairing_request_id: request.pairingRequestId, user_code: userCode,
      poll_secret: request.pollSecret, controller_origin: request.controllerOrigin,
      requested_scopes: request.requestedScopes, expires_at: request.expiresAt, expires_in: 600, interval: 3, email_sent: false,
    }));
    await expect(createPairingRequest(fetcher, relayOrigin, request.controllerOrigin)).rejects.toThrow("invalid pairing code");
  });

  it("executes pending and ready fixtures through the production poll parser", async () => {
    const pendingFixture = await fixture("pending-poll-response");
    const pending = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => response(202, pendingFixture));
    await expect(pollPairingRequest(pending, relayOrigin, request)).resolves.toEqual({ kind: "pending", interval: 3, expiresAt: request.expiresAt });
    expect(String(pending.mock.calls[0][0])).toBe(`${relayOrigin}/api/hub/pairing/requests/poll`);
    expect(JSON.parse(String(pending.mock.calls[0][1]?.body))).toEqual({
      wire_version: 1, pairing_request_id: request.pairingRequestId, poll_secret: request.pollSecret,
    });

    const ready = vi.fn(async () => response(200, await fixture("ready-poll-response")));
    await expect(pollPairingRequest(ready, relayOrigin, request)).resolves.toMatchObject({
      kind: "authorized",
      invitation: { hubUrl: "https://workstation.tail.example", controllerOrigin: request.controllerOrigin },
    });
  });

  it("handles claimed, slow-down, denial, and expiry without leaking relay secrets", async () => {
    const claimed = vi.fn(async () => response(202, { wire_version: 1, status: "machine_claimed", expires_at: request.expiresAt, interval: 3 }));
    await expect(pollPairingRequest(claimed, relayOrigin, request)).resolves.toEqual({ kind: "claimed", interval: 3, expiresAt: request.expiresAt });
    const slow = vi.fn(async () => response(429, { error: "slow_down", error_description: "wait" }, { "Retry-After": "7" }));
    await expect(pollPairingRequest(slow, relayOrigin, request)).resolves.toEqual({ kind: "slow-down", interval: 7 });
    for (const [status, code] of [[403, "request_mismatch"], [410, "expired_request"]] as const) {
      const fetcher = vi.fn(async () => response(status, { error: code, error_description: request.pollSecret }));
      const error = await pollPairingRequest(fetcher, relayOrigin, request).catch((caught) => caught);
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
      { ...base.invitation, hub_url: "https://workstation.tail.example?query=1" },
      { ...base.invitation, hub_url: "https://workstation.tail.example/#fragment" },
      { ...base.invitation, hub_url: "https://user:pass@workstation.tail.example/" },
    ]) {
      const fetcher = vi.fn(async () => response(200, { ...base, invitation }));
      await expect(pollPairingRequest(fetcher, relayOrigin, request)).rejects.toBeInstanceOf(PairingRelayError);
    }
  });

  it.each([
    ["https://workstation.tail.example", "https://workstation.tail.example"],
    ["https://workstation.tail.example/", "https://workstation.tail.example"],
    ["https://workstation.tail.example:443/", "https://workstation.tail.example"],
    ["http://127.0.0.1:4173/", "http://127.0.0.1:4173"],
    ["http://[::1]:4173/", "http://[::1]:4173"],
  ])("normalizes safe root hub URL %s to %s", (value, expected) => {
    expect(normalizeHubOrigin(value)).toBe(expected);
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

  it("consumes a pairing fragment delivered to an already-open tab", () => {
    // Android sends a VIEW intent for a URL that differs only by #fragment to
    // the tab that is already open. Nothing navigates, the SPA never re-inits,
    // and the one-time invitation is dropped without a word.
    const token = "A".repeat(43);
    const store = new PendingPairingStore(new MemoryStorage());
    const listeners = new Map<string, () => void>();
    const location = { hash: "", pathname: "/", search: "" } as Location;
    const history = { replaceState: () => { location.hash = ""; } } as unknown as History;
    const seen: string[] = [];
    const stop = watchPairingFragment(
      {
        location,
        history,
        addEventListener: (type: string, listener: () => void) => { listeners.set(type, listener); },
        removeEventListener: (type: string) => { listeners.delete(type); },
      },
      store,
      (fragment) => { seen.push(fragment.hubId); },
    );

    expect([...listeners.keys()].sort()).toEqual(["focus", "hashchange", "pageshow", "visibilitychange"]);

    location.hash = `#pair=${token}&hub=machine-uuid`;
    listeners.get("hashchange")!();
    expect(seen).toEqual(["machine-uuid"]);
    // The capability is one-time: a second event on the scrubbed URL is a no-op.
    listeners.get("hashchange")!();
    expect(seen).toEqual(["machine-uuid"]);

    // A tab restored from the back-forward cache re-checks on foreground.
    location.hash = `#pair=${token}&hub=second-machine`;
    listeners.get("visibilitychange")!();
    expect(seen).toEqual(["machine-uuid", "second-machine"]);

    stop();
    expect(listeners.size).toBe(0);
  });

  it("leaves an unrelated fragment alone while watching for pairing URLs", () => {
    const location = { hash: "#attention", pathname: "/", search: "" } as Location;
    let replaced = false;
    const history = { replaceState: () => { replaced = true; } } as unknown as History;
    const listeners = new Map<string, () => void>();
    const seen: unknown[] = [];
    watchPairingFragment(
      {
        location,
        history,
        addEventListener: (type: string, listener: () => void) => { listeners.set(type, listener); },
        removeEventListener: () => { /* not exercised here */ },
      },
      new PendingPairingStore(new MemoryStorage()),
      (fragment) => { seen.push(fragment); },
    );
    listeners.get("focus")!();
    expect(seen).toEqual([]);
    // Re-checking on every foreground must not rewrite a hash it does not own.
    expect(replaced).toBe(false);
    expect(location.hash).toBe("#attention");
  });

  it("scrubs a malformed legacy capability even when it cannot be persisted", () => {
    let replacement = "";
    const location = { hash: "#pair=malformed-secret&hub=machine-uuid", pathname: "/commander", search: "?view=fleet" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;
    expect(consumePairingFragment(location, history)).toBeNull();
    expect(replacement).toBe("/commander?view=fleet");
  });

  it.each([
    ["missing hub", `#pair=${"A".repeat(43)}`],
    ["empty-first duplicate pair", `#pair=&pair=${"A".repeat(43)}&hub=machine-uuid`],
    ["truncated pair", `#pair=${"A".repeat(42)}&hub=machine-uuid`],
    ["empty pair", "#pair=&hub=machine-uuid"],
  ])("scrubs a pair-present %s fragment before boot", (_case, hash) => {
    let replacement = "";
    const location = { hash, pathname: "/commander", search: "?view=fleet" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;

    expect(consumePairingFragment(location, history)).toBeNull();
    expect(replacement).toBe("/commander?view=fleet");
  });

  it("scrubs a valid fragment and boots with an in-memory fallback when the sessionStorage getter throws", () => {
    const token = "A".repeat(43);
    let replacement = "";
    const windowLike = Object.defineProperty({}, "sessionStorage", {
      get: () => { throw new DOMException("denied", "SecurityError"); },
    }) as { readonly sessionStorage: Storage };
    const store = pendingPairingStoreFor(windowLike);
    const location = { hash: `#pair=${token}&hub=machine-uuid`, pathname: "/commander", search: "?view=fleet" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;

    expect(() => consumePairingFragment(location, history, store)).not.toThrow();
    expect(replacement).toBe("/commander?view=fleet");
    expect(replacement).not.toContain(token);
    expect(store.load()).toMatchObject({ kind: "invitation", token, hubId: "machine-uuid" });
  });

  it("tolerates throwing sessionStorage get, set, and remove methods", () => {
    const storage = {
      get length(): number { throw new DOMException("denied", "SecurityError"); },
      clear(): void { throw new DOMException("denied", "SecurityError"); },
      getItem(): string | null { throw new DOMException("denied", "SecurityError"); },
      key(): string | null { throw new DOMException("denied", "SecurityError"); },
      removeItem(): void { throw new DOMException("denied", "SecurityError"); },
      setItem(): void { throw new DOMException("denied", "SecurityError"); },
    } satisfies Storage;
    const store = new PendingPairingStore(storage);

    expect(() => store.save(request)).not.toThrow();
    expect(() => store.clear()).not.toThrow();
    expect(store.load()).toBeNull();
  });

  it("surfaces when failed exchange cleanup cannot durably block a reload", () => {
    expect(pairingStorageClearFailureMessage(
      "This pairing invitation has expired or was already used.",
      { persistentRemovalFailed: true, failClosed: false },
    )).toContain("could not durably block");
  });

  it("writes a fail-closed tombstone when only sessionStorage removal is denied", () => {
    const storage = new MemoryStorage();
    storage.removeItem = () => { throw new DOMException("denied", "SecurityError"); };
    const firstPage = new PendingPairingStore(storage);
    firstPage.save(request);

    const result = firstPage.clear();

    expect(result).toMatchObject({ persistentRemovalFailed: true, failClosed: true });
    expect([...storage.values.values()].join("\n")).not.toContain(request.pollSecret);
    expect(new PendingPairingStore(storage).load()).toBeNull();
  });

  it.each(["cancel", "expiry"])("invalidates a deferred poll on %s before it can restore secrets", async () => {
    const operations = new PairingOperationCoordinator();
    const generation = operations.replace();
    const operation = operations.begin(generation);
    const result = deferred<typeof request>();
    const persisted: typeof request[] = [];
    const completion = commitPairingResult(operations, operation, result.promise, async (value) => { persisted.push(value); });

    operations.invalidate();
    result.resolve(request);

    await expect(completion).resolves.toBe(false);
    expect(operation.signal.aborted).toBe(true);
    expect(persisted).toEqual([]);
  });

  it("keeps a replacement request when the superseded deferred poll resolves last", async () => {
    const operations = new PairingOperationCoordinator();
    const firstGeneration = operations.replace();
    const firstOperation = operations.begin(firstGeneration);
    const first = deferred<typeof request>();
    const persisted: string[] = [];
    const staleCompletion = commitPairingResult(operations, firstOperation, first.promise, async (value) => { persisted.push(value.pairingRequestId); });

    const replacement = { ...request, pairingRequestId: "replacement-request" };
    const replacementGeneration = operations.replace();
    const replacementOperation = operations.begin(replacementGeneration);
    await expect(commitPairingResult(operations, replacementOperation, Promise.resolve(replacement), async (value) => { persisted.push(value.pairingRequestId); })).resolves.toBe(true);
    first.resolve(request);

    await expect(staleCompletion).resolves.toBe(false);
    expect(firstOperation.signal.aborted).toBe(true);
    expect(persisted).toEqual(["replacement-request"]);
  });

  it("rolls back a credential persisted after final exchange cancellation and never acknowledges it", async () => {
    const operations = new PairingOperationCoordinator();
    const generation = operations.replace();
    const operation = operations.begin(generation);
    const persistStarted = deferred<void>();
    const releasePersist = deferred<void>();
    const persisted: StoredMachine[] = [];
    const removed: string[] = [];
    const acknowledge = vi.fn(async () => undefined);
    const invitation = { kind: "invitation" as const, token: "one-time-token", hubId: "machine-uuid", hubUrl: "https://workstation.tail.example", controllerOrigin: request.controllerOrigin, scopes: ["machine-read"] as const };
    const exchange = exchangePendingPairing({
      invitation, controllerOrigin: request.controllerOrigin, deviceLabel: "Browser", operatorLabel: "Operator",
      fetcher: async () => response(200, { device_id: "device", credential_id: "credential", credential: "opaque", expires_at: "2027-01-01T00:00:00Z", scopes: ["machine-read"] }),
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: operation.generation,
      stagePersisted: async (machine) => { persisted.push(machine); persistStarted.resolve(); await releasePersist.promise; },
      activatePersisted: async () => true,
      rollbackPersisted: async (identity) => { removed.push(identity.machineId); return true; }, acknowledge,
      signal: operation.signal, isCurrent: () => operations.isCurrent(operation),
    });

    await persistStarted.promise;
    operations.invalidate();
    releasePersist.resolve();

    await expect(exchange).rejects.toBeInstanceOf(PairingExchangeError);
    expect(persisted).toHaveLength(1);
    expect(removed).toEqual([invitation.hubId]);
    expect(acknowledge).not.toHaveBeenCalled();
  });

  it("surfaces the hub's exchange error instead of a generic expired invitation", async () => {
    const invitation = { kind: "invitation" as const, token: "one-time-token", hubId: "machine-uuid", hubUrl: "https://workstation.tail.example", controllerOrigin: request.controllerOrigin, scopes: ["machine-read"] as const };
    await expect(exchangePendingPairing({
      invitation, controllerOrigin: request.controllerOrigin, deviceLabel: "Browser", operatorLabel: "Operator",
      fetcher: async () => response(403, { error: "This device is not approved for this hub." }),
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: async () => undefined, activatePersisted: async () => true, rollbackPersisted: async () => true,
    })).rejects.toThrow("This device is not approved for this hub.");
  });

  it("keeps a lost-response invitation for a bounded retry without claiming nonconsumption", async () => {
    const invitation = { kind: "invitation" as const, token: "one-time-token", hubId: "machine-uuid", hubUrl: "https://workstation.tail.example", controllerOrigin: request.controllerOrigin, scopes: ["machine-read"] as const };
    let consumed = false;
    let attempts = 0;
    const run = () => exchangePendingPairing({
      invitation, controllerOrigin: request.controllerOrigin, deviceLabel: "Browser", operatorLabel: "Operator",
      fetcher: async () => {
        attempts += 1;
        if (attempts === 1) {
          consumed = true;
          throw new TypeError("response lost after the machine consumed the invitation");
        }
        return response(401, { error: "unauthorized" });
      },
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: attempts + 1,
      stagePersisted: async () => undefined, activatePersisted: async () => true, rollbackPersisted: async () => true,
    });

    const uncertain = await exchangeFailure(run());
    expect(consumed).toBe(true);
    expect(uncertain.recoverable).toBe(true);
    expect(uncertain.message).toMatch(/could not confirm pairing/i);
    expect(uncertain.message).toMatch(/fresh invitation/i);
    expect(uncertain.message).not.toMatch(/still open|unused|untouched/i);

    const refused = await exchangeFailure(run());
    expect(refused.recoverable).toBe(false);
    expect(attempts).toBe(2);
  });

  it("keeps a cleanup-rejected credential non-activatable after reload, then restores the exact prior row", async () => {
    const backend = new MemoryMachineCatalogBackend();
    const catalog = new MachineCatalog(backend);
    const prior = storedMachine("prior");
    const candidate = storedMachine("cancelled");
    const identity = installIdentity(candidate.credentialId, 7);
    backend.records.set(prior.id, prior);

    await catalog.stage(candidate, identity);
    backend.rejectUpdate = true;
    await expect(catalog.rollback(identity)).rejects.toThrow("durable cleanup rejected");

    const reloaded = new MachineCatalog(backend);
    await expect(reloaded.snapshot()).resolves.toEqual({ machines: [prior], pendingCleanup: 1 });
    backend.rejectUpdate = false;
    await expect(reloaded.recoverPending()).resolves.toEqual({ machines: [prior], pendingCleanup: 0 });
    expect(backend.records.get(prior.id)).toEqual(prior);
  });

  it("writes activated records in the prior reader's StoredMachine shape", async () => {
    const backend = new MemoryMachineCatalogBackend();
    const catalog = new MachineCatalog(backend);
    const candidate = storedMachine("active");
    const identity = installIdentity(candidate.credentialId, 8);

    await catalog.stage(candidate, identity);
    await expect(catalog.activate(identity)).resolves.toBe(true);

    // The pre-catalog Commander bundle reads the v1 store directly as StoredMachine rows.
    const priorMainReader = async (): Promise<StoredMachine[]> => [...backend.records.values()] as StoredMachine[];
    await expect(priorMainReader()).resolves.toMatchObject([candidate]);
  });

  it("keeps cancellation fail-closed after activation when its first rollback write rejects", async () => {
    const backend = new MemoryMachineCatalogBackend();
    const catalog = new MachineCatalog(backend);
    const prior = storedMachine("prior");
    const candidate = storedMachine("cancelled-after-activation");
    const identity = installIdentity(candidate.credentialId, 9);
    backend.records.set(prior.id, prior);

    await catalog.stage(candidate, identity);
    await catalog.activate(identity);
    backend.rejectNextUpdate = true;

    await expect(catalog.rollback(identity)).rejects.toThrow("durable cleanup rejected");
    const reloaded = new MachineCatalog(backend);
    await expect(reloaded.snapshot()).resolves.toEqual({ machines: [prior], pendingCleanup: 1 });
  });

  it("reports active cancellation as blocked after a transient rollback failure and keeps it hidden on reload", async () => {
    const backend = new MemoryMachineCatalogBackend();
    const catalog = new MachineCatalog(backend);
    const prior = storedMachine("prior");
    const identity = installIdentity("cancelled-after-activation", 10);
    backend.records.set(prior.id, prior);
    let current = true;

    const exchange = exchangePendingPairing({
      invitation: { kind: "invitation", token: "one-time-token", hubId: identity.machineId, hubUrl: "https://workstation.tail.example", controllerOrigin: request.controllerOrigin, scopes: ["machine-read"] },
      controllerOrigin: request.controllerOrigin, deviceLabel: "Browser", operatorLabel: "Operator",
      fetcher: async () => response(200, { device_id: "device", credential_id: identity.credentialId, credential: "opaque", expires_at: "2027-01-01T00:00:00Z", scopes: ["machine-read"] }),
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: identity.generation,
      stagePersisted: (machine, install) => catalog.stage(machine, install),
      activatePersisted: async (install) => {
        const activated = await catalog.activate(install);
        current = false;
        backend.rejectNextUpdate = true;
        return activated;
      },
      rollbackPersisted: (install) => catalog.rollback(install),
      isCurrent: () => current,
    });

    await expect(exchange).rejects.toBeInstanceOf(PairingCleanupError);
    await expect(new MachineCatalog(backend).snapshot()).resolves.toEqual({ machines: [prior], pendingCleanup: 1 });
  });

  it("lets only the exact same-hub credential generation activate or roll back", async () => {
    const backend = new MemoryMachineCatalogBackend();
    const catalog = new MachineCatalog(backend);
    const prior = storedMachine("prior");
    const first = storedMachine("first");
    const replacement = storedMachine("replacement");
    const firstIdentity = installIdentity(first.credentialId, 8);
    const replacementIdentity = installIdentity(replacement.credentialId, 9);
    backend.records.set(prior.id, prior);

    await catalog.stage(first, firstIdentity);
    await catalog.stage(replacement, replacementIdentity);
    await expect(catalog.rollback(firstIdentity)).resolves.toBe(false);
    await expect(catalog.activate(firstIdentity)).resolves.toBe(false);
    await expect(catalog.snapshot()).resolves.toEqual({ machines: [prior], pendingCleanup: 1 });
    await expect(catalog.activate(replacementIdentity)).resolves.toBe(true);
    await expect(catalog.snapshot()).resolves.toEqual({ machines: [replacement], pendingCleanup: 0 });
  });

  it("surfaces rejected post-persist cleanup while leaving reload fail-closed", async () => {
    const operations = new PairingOperationCoordinator();
    const operation = operations.begin(operations.replace());
    const backend = new MemoryMachineCatalogBackend();
    const catalog = new MachineCatalog(backend);
    const persistStarted = deferred<void>();
    const releasePersist = deferred<void>();
    const invitation = { kind: "invitation" as const, token: "one-time-token", hubId: "machine-uuid", hubUrl: "https://workstation.tail.example", controllerOrigin: request.controllerOrigin, scopes: ["machine-read"] as const };
    const identity = installIdentity("credential", operation.generation);
    const exchange = exchangePendingPairing({
      invitation, controllerOrigin: request.controllerOrigin, deviceLabel: "Browser", operatorLabel: "Operator",
      fetcher: async () => response(200, { device_id: "device", credential_id: identity.credentialId, credential: "opaque", expires_at: "2027-01-01T00:00:00Z", scopes: ["machine-read"] }),
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: operation.generation,
      stagePersisted: async (machine, install) => { await catalog.stage(machine, install); persistStarted.resolve(); await releasePersist.promise; },
      activatePersisted: (install, signal) => catalog.activate(install, signal),
      rollbackPersisted: (install) => catalog.rollback(install),
      signal: operation.signal, isCurrent: () => operations.isCurrent(operation),
    });

    await persistStarted.promise;
    operations.invalidate();
    backend.rejectDelete = true;
    releasePersist.resolve();

    await expect(exchange).rejects.toMatchObject({ name: "PairingCleanupError" });
    await expect(catalog.snapshot()).resolves.toMatchObject({ machines: [], pendingCleanup: 1 });
  });

  it("keeps a replacement request and draft when an older cleanup rejection settles last", async () => {
    const operations = new PairingOperationCoordinator();
    const operation = operations.begin(operations.replace());
    const persistStarted = deferred<void>();
    const releasePersist = deferred<void>();
    const oldInvitation = {
      kind: "invitation" as const,
      token: "old-one-time-token",
      hubId: "machine-uuid",
      hubUrl: "https://workstation.tail.example",
      controllerOrigin: request.controllerOrigin,
      scopes: ["machine-read"] as const,
    };
    const exchange = exchangePendingPairing({
      invitation: oldInvitation, controllerOrigin: request.controllerOrigin, deviceLabel: "Browser", operatorLabel: "Operator",
      fetcher: async () => response(200, { device_id: "old-device", credential_id: "old-credential", credential: "old-opaque", expires_at: "2027-01-01T00:00:00Z", scopes: ["machine-read"] }),
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: operation.generation,
      stagePersisted: async () => { persistStarted.resolve(); await releasePersist.promise; },
      activatePersisted: async () => true,
      rollbackPersisted: async () => { throw new Error("durable cleanup rejected"); },
      signal: operation.signal,
      isCurrent: () => operations.isCurrent(operation),
    });

    await persistStarted.promise;
    operations.invalidate();
    const replacement = { ...request, pairingRequestId: "replacement-request", pollSecret: "replacement-poll-secret" };
    operations.replace();
    const replacementDraft = { ...createPairingDraft(request.controllerOrigin), email: "replacement@example.com" };
    const current = {
      pendingPairing: replacement,
      pairingDraft: replacementDraft,
      exchangeInFlight: false,
      status: "Waiting for the replacement machine…",
    };
    releasePersist.resolve();

    const cleanupError = await exchange.catch((error: unknown) => error);
    expect(cleanupError).toBeInstanceOf(PairingCleanupError);
    expect(pairingCleanupFailureUpdate({
      coordinator: operations,
      operation,
      expectedPending: oldInvitation,
      current,
      cleanupMessage: (cleanupError as Error).message,
      resetDraft: () => createPairingDraft(request.controllerOrigin),
    })).toBeNull();
    expect(current.pendingPairing).toEqual(replacement);
    expect(current.pendingPairing.pollSecret).toBe("replacement-poll-secret");
    expect(current.pairingDraft).toEqual(replacementDraft);
    expect(current.status).toBe("Waiting for the replacement machine…");
  });

  it("applies a cleanup failure only to its current pending invitation", () => {
    const operations = new PairingOperationCoordinator();
    const operation = operations.begin(operations.replace());
    const invitation = {
      kind: "invitation" as const,
      token: "one-time-token",
      hubId: "machine-uuid",
    };
    const current = {
      pendingPairing: invitation,
      pairingDraft: { ...createPairingDraft(request.controllerOrigin), email: "current@example.com" },
      exchangeInFlight: true,
      status: "Creating this browser credential…",
    };

    expect(pairingCleanupFailureUpdate({
      coordinator: operations,
      operation,
      expectedPending: invitation,
      current,
      cleanupMessage: "durable cleanup is pending",
      resetDraft: () => createPairingDraft(request.controllerOrigin),
    })).toEqual({
      pendingPairing: null,
      pairingDraft: createPairingDraft(request.controllerOrigin),
      exchangeInFlight: false,
      status: "durable cleanup is pending",
    });
  });

  it("does not persist a credential when a deferred exchange response arrives after cancellation", async () => {
    const operations = new PairingOperationCoordinator();
    const generation = operations.replace();
    const operation = operations.begin(generation);
    const fetched = deferred<Response>();
    const persist = vi.fn(async (_machine: StoredMachine) => undefined);
    const invitation = { kind: "invitation" as const, token: "one-time-token", hubId: "machine-uuid", hubUrl: "https://workstation.tail.example", controllerOrigin: request.controllerOrigin, scopes: ["machine-read"] as const };
    const exchange = exchangePendingPairing({
      invitation, controllerOrigin: request.controllerOrigin, deviceLabel: "Browser", operatorLabel: "Operator",
      fetcher: async () => fetched.promise,
      createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: operation.generation,
      stagePersisted: persist,
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
      signal: operation.signal, isCurrent: () => operations.isCurrent(operation),
    });

    await Promise.resolve();
    operations.invalidate();
    fetched.resolve(response(200, { device_id: "device", credential_id: "credential", credential: "opaque", expires_at: "2027-01-01T00:00:00Z", scopes: ["machine-read"] }));

    await expect(exchange).rejects.toBeInstanceOf(PairingExchangeError);
    expect(persist).not.toHaveBeenCalled();
  });

  it.each([
    ["legacy", [["url", "https://legacy.tail.example"], ["label", "Studio Mac"], ["device", "Phone"], ["operator", "Petra"], ["scope", "machine-read"], ["scope", "pane-read"]]],
    ["relay", [["device", "Tablet"], ["operator", "Daniel"]]],
  ] as const)("preserves %s confirmation fields across unrelated renders", (_kind, entries) => {
    const initial = createPairingDraft("https://controller.tail.example");
    // The machine's address is never guessed from the page (cas-8051 F5); the
    // page origin is kept only as the one-tap fill for the served-by-hub case.
    expect(initial.hubUrl).toBe("");
    expect(initial.pageOrigin).toBe("https://controller.tail.example");
    const captured = updatePairingDraft(initial, entries);
    const afterBackgroundRender = updatePairingDraft(captured, []);

    expect(afterBackgroundRender).toEqual(captured);
    expect(afterBackgroundRender.deviceLabel).not.toBe("Cassy Commander browser");
  });

  it.each([
    ["create", { createInFlight: true, exchangeInFlight: false, hasPendingPairing: false }],
    ["poll", { createInFlight: false, exchangeInFlight: false, hasPendingPairing: true }],
    ["persist", { createInFlight: false, exchangeInFlight: true, hasPendingPairing: true }],
    ["ack", { createInFlight: false, exchangeInFlight: true, hasPendingPairing: true }],
  ] as const)("routes native dialog Escape through cancellation during deferred %s", (_phase, state) => {
    const dialog = new EventTarget() as HTMLDialogElement;
    const cancel = vi.fn();
    bindPairingDialogCancel(dialog, () => state, cancel);

    const event = new Event("cancel", { cancelable: true });
    dialog.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(true);
    expect(cancel).toHaveBeenCalledOnce();
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
      fetcher, createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: persist,
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
      acknowledge: (relay) => acknowledgePairing(fetcher, relayOrigin, relay),
    })).resolves.toMatchObject({ id: invitation.hubId, baseUrl: invitation.hubUrl });
    expect(events).toEqual(["persist", "ack"]);
    const [hubTarget, hubInit] = fetcher.mock.calls[0];
    expect(String(hubTarget)).toBe("https://workstation.tail.example/v1/auth/pairing/exchange");
    expect(JSON.parse(String(hubInit?.body))).toMatchObject({ controller_origin: request.controllerOrigin, requested_scopes: ["machine-read"] });
  });

  it("completes final pairing and leaves a usable active machine record", async () => {
    const backend = new MemoryMachineCatalogBackend();
    const catalog = new MachineCatalog(backend);
    const privateKey = {} as CryptoKey;
    const invitation = {
      kind: "invitation" as const,
      token: "one-time-token",
      hubId: "machine-uuid",
      hubUrl: "https://workstation.tail.example",
      machineLabel: "Studio workstation",
      controllerOrigin: request.controllerOrigin,
      scopes: ["machine-read"] as const,
    };

    const machine = await exchangePendingPairing({
      invitation,
      controllerOrigin: request.controllerOrigin,
      deviceLabel: "Browser",
      operatorLabel: "Operator",
      fetcher: async () => response(200, {
        device_id: "device",
        credential_id: "credential",
        credential: "opaque",
        expires_at: "2027-01-01T00:00:00Z",
        scopes: ["machine-read"],
      }),
      createKey: async () => ({ privateKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: (candidate, identity) => catalog.stage(candidate, identity),
      activatePersisted: (identity, signal) => catalog.activate(identity, signal),
      rollbackPersisted: (identity) => catalog.rollback(identity),
    });

    expect(machine).toMatchObject({
      id: invitation.hubId,
      baseUrl: invitation.hubUrl,
      credentialId: "credential",
      privateKey,
    });
    await expect(catalog.snapshot()).resolves.toEqual({ machines: [machine], pendingCleanup: 0 });
  });

  it("serializes acknowledgement from the shared fixture to the explicit relay", async () => {
    const acknowledge = await fixture("acknowledge-request");
    const fetcher = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => response(204));
    await acknowledgePairing(fetcher, relayOrigin, {
      pairingRequestId: String(acknowledge.pairing_request_id),
      pollSecret: String(acknowledge.poll_secret),
      deliveryId: String(acknowledge.delivery_id),
    });
    const [target, init] = fetcher.mock.calls[0];
    expect(String(target)).toBe(`${relayOrigin}/api/hub/pairing/requests/acknowledge`);
    expect(JSON.parse(String(init?.body))).toEqual(acknowledge);
    expect(init).toMatchObject({ method: "POST", credentials: "omit" });
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
      fetcher, createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
      installationGeneration: 1,
      stagePersisted: async (machine) => { persisted.push(machine); },
      activatePersisted: async () => true,
      rollbackPersisted: async () => true,
    });
    const settled = await Promise.allSettled([run(), run()]);
    expect(settled.map((result) => result.status).sort()).toEqual(["fulfilled", "rejected"]);
    expect((settled.find((result) => result.status === "rejected") as PromiseRejectedResult).reason).toBeInstanceOf(PairingExchangeError);
    expect(persisted).toHaveLength(1);
  });
});
