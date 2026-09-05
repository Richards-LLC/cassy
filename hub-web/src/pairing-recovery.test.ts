import { describe, expect, it, vi } from "vitest";
import { readPairingFragment, watchPairingFragment } from "./fragment";
import { EXPIRED_PAIRING_INVITATION_MESSAGE, INVALID_PAIRING_LINK_MESSAGE, cancellationOutcome } from "./pairing-cleanup";
import { exchangePendingPairing, PairingCleanupError, PairingExchangeError, PairingStorageError } from "./pairing-exchange";
import { PendingPairingStore } from "./pending-pairing";

class MemoryStorage implements Storage {
  readonly values = new Map<string, string>();
  get length(): number { return this.values.size; }
  clear(): void { this.values.clear(); }
  getItem(key: string): string | null { return this.values.get(key) ?? null; }
  key(index: number): string | null { return [...this.values.keys()][index] ?? null; }
  removeItem(key: string): void { this.values.delete(key); }
  setItem(key: string, value: string): void { this.values.set(key, value); }
}

const TOKEN = "A".repeat(43);

function response(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

const credential = { device_id: "device", credential_id: "credential", credential: "opaque", expires_at: "2030-01-01T00:00:00Z", scopes: ["machine-read"] };

function exchange(overrides: Partial<Parameters<typeof exchangePendingPairing>[0]>) {
  return exchangePendingPairing({
    invitation: { kind: "invitation", token: TOKEN, hubId: "machine-uuid", hubUrl: "https://workstation.tail.example", controllerOrigin: "https://commander.example", scopes: ["machine-read"] },
    controllerOrigin: "https://commander.example",
    deviceLabel: "Phone",
    operatorLabel: "Operator",
    fetcher: async () => response(200, credential),
    createKey: async () => ({ privateKey: {} as CryptoKey, publicKey: { kty: "EC" } }),
    installationGeneration: 1,
    stagePersisted: async () => undefined,
    activatePersisted: async () => true,
    rollbackPersisted: async () => true,
    ...overrides,
  });
}

describe("F6: invalid and expired links keep their nonsecret outcome", () => {
  it("reports a malformed #pair fragment as invalid, scrubs it, and never carries the token", () => {
    let replacement = "";
    const location = { hash: "#pair=not-a-token&hub=machine-uuid", pathname: "/commander", search: "" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;
    const outcome = readPairingFragment(location, history);
    expect(outcome).toEqual({ kind: "invalid" });
    expect(replacement).toBe("/commander");
    expect(JSON.stringify(outcome)).not.toContain("not-a-token");
    expect(INVALID_PAIRING_LINK_MESSAGE).not.toContain(TOKEN);
  });

  it("returns none for a URL without a pairing fragment and the fragment for a valid one", () => {
    const history = { replaceState: () => undefined } as unknown as History;
    expect(readPairingFragment({ hash: "", pathname: "/", search: "" } as Location, history)).toEqual({ kind: "none" });
    expect(readPairingFragment({ hash: `#pair=${TOKEN}&hub=machine-uuid`, pathname: "/", search: "" } as Location, history))
      .toEqual({ kind: "fragment", fragment: { token: TOKEN, hubId: "machine-uuid" } });
  });

  it("tells the watcher about an invalid link delivered to an open tab", () => {
    const listeners = new Map<string, () => void>();
    const location = { hash: "", pathname: "/", search: "" } as { hash: string; pathname: string; search: string };
    const target = {
      location: location as Location,
      history: { replaceState: (_: unknown, __: string, path: string) => { location.hash = ""; void path; } } as unknown as History,
      addEventListener: (type: string, listener: () => void) => listeners.set(type, listener),
      removeEventListener: (type: string) => listeners.delete(type),
    };
    const store = new PendingPairingStore(new MemoryStorage());
    const onFragment = vi.fn();
    const onInvalid = vi.fn();
    watchPairingFragment(target, store, onFragment, onInvalid);
    location.hash = "#pair=broken&hub=machine-uuid";
    listeners.get("hashchange")!();
    expect(onInvalid).toHaveBeenCalledTimes(1);
    expect(onFragment).not.toHaveBeenCalled();
    expect(location.hash).toBe("");
    expect(store.load()).toBeNull();
  });

  it("distinguishes an expired stored invitation from an empty store and clears it", () => {
    const storage = new MemoryStorage();
    const now = Date.parse("2026-09-05T00:00:00Z");
    const live = new PendingPairingStore(storage, () => now - 60_000);
    live.save({ kind: "invitation", token: TOKEN, hubId: "machine-uuid", expiresAt: "2026-09-05T00:00:00Z" });
    const later = new PendingPairingStore(storage, () => now);
    expect(later.loadOutcome()).toEqual({ kind: "expired" });
    expect(storage.values.size).toBe(0);
    expect(later.loadOutcome()).toEqual({ kind: "none" });
    expect(EXPIRED_PAIRING_INVITATION_MESSAGE).not.toContain(TOKEN);
  });
});

describe("F3: storage failure after the hub consumed the invitation", () => {
  it("is its own error, after a successful exchange, with the staged row rolled back", async () => {
    const rollback = vi.fn(async () => true);
    const attempt = exchange({
      stagePersisted: async () => { throw new DOMException("Fixture storage denied", "QuotaExceededError"); },
      rollbackPersisted: rollback,
    });
    await expect(attempt).rejects.toBeInstanceOf(PairingStorageError);
    await expect(attempt).rejects.toThrow("could not save access");
    await expect(attempt).rejects.not.toThrow("expired or was already used");
    // Nothing was staged, so there is nothing to roll back.
    expect(rollback).not.toHaveBeenCalled();
  });

  it("covers activation as well as staging, and still rolls the staged row back", async () => {
    const rollback = vi.fn(async () => true);
    const attempt = exchange({ activatePersisted: async () => { throw new Error("IndexedDB unavailable"); }, rollbackPersisted: rollback });
    await expect(attempt).rejects.toBeInstanceOf(PairingStorageError);
    expect(rollback).toHaveBeenCalledTimes(1);
  });

  it("is not recoverable: the invitation is consumed and a fresh one is the next step", async () => {
    const error = await exchange({ stagePersisted: async () => { throw new Error("denied"); } }).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(PairingExchangeError);
    expect((error as PairingExchangeError).recoverable).toBe(false);
    expect(String((error as Error).message)).not.toContain("denied");
  });

  it("keeps supersession and cancellation as their own errors", async () => {
    await expect(exchange({ activatePersisted: async () => false })).rejects.toThrow("superseded");
    const controller = new AbortController();
    await expect(exchange({
      signal: controller.signal,
      stagePersisted: async () => { controller.abort(); throw new DOMException("aborted", "AbortError"); },
    })).rejects.not.toBeInstanceOf(PairingStorageError);
  });

  it("still reports a failed rollback as a cleanup error", async () => {
    await expect(exchange({
      activatePersisted: async () => { throw new Error("IndexedDB unavailable"); },
      rollbackPersisted: async () => { throw new Error("rollback rejected"); },
    })).rejects.toBeInstanceOf(PairingCleanupError);
  });
});

describe("F2: cancellation says whether it is durable", () => {
  it("keeps the dialog open with a retry when storage could not record the cancellation", () => {
    const outcome = cancellationOutcome({ persistentRemovalFailed: true, failClosed: false }, false);
    expect(outcome.cleanupFailed).toBe(true);
    expect(outcome.status).toContain("retry the cleanup");
    expect(outcome.status).not.toContain("resume");
  });

  it("stays visible while an in-flight exchange unwinds", () => {
    const outcome = cancellationOutcome({ persistentRemovalFailed: false, failClosed: true }, true);
    expect(outcome).toMatchObject({ cleanupFailed: false, verifying: true });
  });

  it("closes on a durable cancellation, naming the tombstone when removal was denied", () => {
    expect(cancellationOutcome({ persistentRemovalFailed: false, failClosed: true }, false)).toEqual({ cleanupFailed: false, verifying: false, status: "Pairing cancelled." });
    expect(cancellationOutcome({ persistentRemovalFailed: true, failClosed: true }, false).status).toContain("durably blocked");
  });
});
