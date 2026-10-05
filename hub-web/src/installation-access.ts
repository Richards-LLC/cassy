import { anySignal } from "./abort-signals";
import { createDeviceKey } from "./dpop";
import { exchangePendingPairing, PairingCleanupError, PairingExchangeError, type ExchangeOptions } from "./pairing-exchange";
import type { MachineCatalog } from "./storage";
import type { PairingInstallIdentity, StoredMachine, Scope } from "./types";

export type AccountEnrollment = { state: "unenrolled" };
export type InstallationKey = Awaited<ReturnType<typeof createDeviceKey>>;
export interface InstallationRecord extends InstallationKey {
  id: string;
  known?: { deviceId: string; credentialGeneration: number };
  pending?: { operationId: string; baseUrl: string; controllerOrigin: string; invitationHash: string; previousKey?: InstallationKey; identity?: PairingInstallIdentity };
}
export interface InstallationStore {
  get(id: string): Promise<InstallationRecord | undefined>;
  put(record: InstallationRecord): Promise<unknown>;
  list(): Promise<InstallationRecord[]>;
}
export type InstallationLock = <T>(hubId: string, run: () => Promise<T>, signal?: AbortSignal) => Promise<T>;
export const installationLock: InstallationLock = async (hubId, run, signal) => {
  if (!navigator.locks) return Promise.reject(new Error("This browser cannot safely coordinate pairing across tabs. Use a browser with Web Locks support."));
  return navigator.locks.request(`cassy-installation:${hubId}`, { mode: "exclusive", ...(signal ? { signal } : {}) }, run);
};
const encoder = new TextEncoder();
// A signal alone cannot bound a stuck signing/fetch/body promise. Late results
// cannot reach the caller or advance the local installation transaction.
export async function installationDeadline<T>(run: (signal: AbortSignal) => Promise<T>, parent?: AbortSignal): Promise<T> {
  const controller = new AbortController();
  const signal = parent ? anySignal([parent, controller.signal]) : controller.signal;
  let rejectAbort!: (error: unknown) => void;
  const aborted = new Promise<never>((_, reject) => { rejectAbort = reject; });
  const onAbort = () => rejectAbort(signal.reason ?? new DOMException("Installation request cancelled.", "AbortError"));
  signal.addEventListener("abort", onAbort, { once: true });
  const timer = setTimeout(() => controller.abort(new DOMException("The hub did not confirm installation access within 10 seconds. Retry cleanup before pairing again.", "TimeoutError")), 10_000);
  try {
    if (signal.aborted) { onAbort(); return await aborted; }
    return await Promise.race([run(signal), aborted]);
  } finally { clearTimeout(timer); signal.removeEventListener("abort", onAbort); }
}
export function b64url(bytes: ArrayBuffer | Uint8Array): string {
  return btoa(String.fromCharCode(...new Uint8Array(bytes))).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}
export async function installationHash(value: string): Promise<string> {
  return b64url(await crypto.subtle.digest("SHA-256", encoder.encode(value)));
}
export async function signInstallation(key: CryptoKey, transcript: unknown[]): Promise<string> {
  return b64url(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, encoder.encode(JSON.stringify(transcript))));
}
const scopeOrder: Scope[] = ["machine-read", "session-read", "session-launch", "pane-read", "pane-input", "message-send", "pane-interrupt", "factory-operate", "factory-manage", "hub-admin"];
export async function installationTranscript(options: ExchangeOptions, record: InstallationRecord, machine: Pick<StoredMachine, "deviceId" | "credentialGeneration"> | undefined, operationId: string, credential: string, scopes: readonly Scope[]): Promise<unknown[]> {
  const key = record.publicKey;
  const thumbprint = await installationHash(JSON.stringify({ crv: key.crv, kty: key.kty, x: key.x, y: key.y }));
  return ["cassy-installation-v1", options.invitation.hubId, options.controllerOrigin,
    await installationHash(options.invitation.token), operationId, machine?.deviceId ?? null,
    machine?.credentialGeneration ?? 0, thumbprint, await installationHash(credential),
    scopeOrder.filter((scope) => scopes.includes(scope)), options.deviceLabel, options.operatorLabel];
}

/** Owns durable key retention and serializes every installation credential mutation. */
export class InstallationAccess {
  constructor(private readonly store: InstallationStore, private readonly catalog: MachineCatalog,
    private readonly lock: InstallationLock = installationLock, private readonly createKey = createDeviceKey) {}

  async forgetRevoked(hubId: string, baseUrl: string, deviceId: string): Promise<void> {
    await this.lock(hubId, async () => {
      const record = await this.store.get(`${hubId}@${new URL(baseUrl).origin}`);
      if (record?.pending) throw new Error("Access was revoked, but another pairing still needs cleanup. Retry cleanup before enrolling again.");
      if (record?.known?.deviceId === deviceId) {
        delete record.known;
        await this.store.put(record);
      }
    });
  }

  async pair(options: ExchangeOptions): Promise<StoredMachine> {
    if (options.invitation.controllerOrigin && options.invitation.controllerOrigin !== options.controllerOrigin) {
      throw new PairingExchangeError("This pairing invitation belongs to a different Cassy Cloud origin.");
    }
    return this.lock(options.invitation.hubId, async () => {
      await this.recoverHub(options.invitation.hubId, options.fetcher);
      const baseUrl = options.invitation.hubUrl ?? new URL(options.legacyHubUrl!).origin;
      // Old hubs ignore unknown exchange fields and would enroll immediately.
      // Check support before sending a capability that they could consume.
      let protocol: number | undefined;
      try {
        protocol = await installationDeadline(async (signal) => {
          const support = await options.fetcher(new URL("/v1/auth/pairing/protocol", baseUrl), {
            method: "POST", credentials: "omit", cache: "no-store", signal, headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ controller_origin: options.controllerOrigin, pairing_token_hash: await installationHash(options.invitation.token) }),
          });
          if (!support.ok) throw new Error("hub health refused");
          return (await support.json() as { installation_protocol?: number }).installation_protocol;
        }, options.signal);
      } catch { throw new PairingExchangeError("Cannot reach the hub to check safe installation rotation. Check its address and browser network permission, then retry.", { recoverable: true }); }
      if (protocol !== 1) throw new PairingExchangeError("Update this hub before pairing: it does not support safe installation rotation.", { recoverable: true });
      const id = `${options.invitation.hubId}@${new URL(baseUrl).origin}`;
      const prior = (await this.catalog.snapshot()).machines.find((m) => m.id === options.invitation.hubId && new URL(m.baseUrl).origin === new URL(baseUrl).origin);
      let record = await this.store.get(id);
      if (!record) {
        record = { id, ...(prior ? { privateKey: prior.privateKey, publicKey: prior.publicKey } : await installationDeadline(() => this.createKey(), options.signal)) };
        options.signal?.throwIfAborted();
        if (options.isCurrent?.() === false) throw new PairingExchangeError("Pairing was cancelled before access could be saved.");
        await this.store.put(record);
      }
      const previousKey = options.rotateKey ? { privateKey: record.privateKey, publicKey: record.publicKey } : undefined;
      if (previousKey) record = { ...record, ...await installationDeadline(() => this.createKey(), options.signal) };
      const operationId = crypto.randomUUID();
      const credential = b64url(crypto.getRandomValues(new Uint8Array(32)));
      const ceiling = options.invitation.scopes ?? options.requestedScopes ?? [];
      const scopes = options.requestedScopes ? options.requestedScopes.filter((scope) => ceiling.includes(scope)) : ceiling;
      const known = prior ?? record.known;
      const transcript = await installationTranscript(options, record, known, operationId, credential, scopes);
      const proof = await signInstallation(record.privateKey, transcript);
      options.signal?.throwIfAborted();
      if (options.isCurrent?.() === false) throw new PairingExchangeError("Pairing was cancelled before access could be saved.");
      record.pending = { operationId, baseUrl, controllerOrigin: options.controllerOrigin, invitationHash: await installationHash(options.invitation.token), previousKey };
      await this.store.put(record); // Before POST: recovery can cancel even a lost response.
      const current = record;
      try {
        const machine = await exchangePendingPairing({ ...options,
          createKey: async () => current,
          installation: { operation_id: operationId, credential, device_id: known?.deviceId ?? null,
            expected_generation: known?.credentialGeneration ?? 0, proof, previous_proof: previousKey ? await signInstallation(previousKey.privateKey, transcript) : null },
          stagePersisted: async (candidate, identity) => {
            if (candidate.credential !== credential || !candidate.credentialGeneration) throw new PairingExchangeError("The hub does not support safe installation rotation. Update it before pairing.");
            current.pending!.identity = identity;
            await this.store.put(current);
            return options.stagePersisted(candidate, identity);
          },
          commitPrepared: () => this.action(current, "commit", options.fetcher, options.signal),
          // Remote restoration must complete before the local prior can become active.
          abortPrepared: () => this.action(current, "abort", options.fetcher, AbortSignal.timeout(10_000)),
        });
        const completed = { ...current, known: { deviceId: machine.deviceId, credentialGeneration: machine.credentialGeneration! } };
        delete completed.pending;
        await this.store.put(completed);
        Object.assign(current, completed);
        delete current.pending;
        notifyInstallation(machine.id);
        return machine;
      } catch (error) {
        // This also covers cancellation before response decoding or before catalog staging.
        try {
          await this.action(current, "abort", options.fetcher, AbortSignal.timeout(10_000));
          if (current.pending?.identity) await this.catalog.rollback(current.pending.identity);
          if (current.pending?.previousKey) Object.assign(current, current.pending.previousKey);
          delete current.pending;
          await this.store.put(current);
        } catch (cleanup) { throw new PairingCleanupError(cleanup); }
        throw error;
      }
    }, options.signal);
  }

  private async action(record: InstallationRecord, action: "commit" | "abort", fetcher: ExchangeOptions["fetcher"], signal?: AbortSignal): Promise<void> {
    const pending = record.pending;
    if (!pending) return;
    await installationDeadline(async (requestSignal) => {
      const proof = await signInstallation(record.privateKey, [`cassy-installation-${action}-v1`, record.id.split("@")[0], pending.controllerOrigin, pending.operationId, pending.invitationHash]);
      requestSignal.throwIfAborted();
      const response = await fetcher(new URL(`/v1/auth/pairing/${action}`, pending.baseUrl), {
        method: "POST", credentials: "omit", headers: { "Content-Type": "application/json" }, signal: requestSignal,
        body: JSON.stringify({ operation_id: pending.operationId, controller_origin: pending.controllerOrigin, public_key_jwk: record.publicKey, pairing_token_hash: pending.invitationHash, proof }),
      });
      if (!response.ok) throw new Error("Installation recovery needs the hub to confirm its credential generation. Retry cleanup before pairing again.");
    }, signal);
  }

  private async recoverHub(hubId: string, fetcher: ExchangeOptions["fetcher"]): Promise<void> {
    for (const record of await this.store.list()) {
      if (!record.pending || record.id.split("@")[0] !== hubId) continue;
      await this.action(record, "abort", fetcher, AbortSignal.timeout(10_000));
      if (record.pending.identity) await this.catalog.rollback(record.pending.identity);
      if (record.pending.previousKey) Object.assign(record, record.pending.previousKey);
      delete record.pending;
      await this.store.put(record);
    }
  }
  async recover(fetcher: ExchangeOptions["fetcher"]): Promise<number> {
    let pending = 0;
    const hubs = new Set((await this.store.list()).filter((r) => r.pending).map((r) => r.id.split("@")[0]));
    for (const hubId of hubs) {
      try { await this.lock(hubId, () => this.recoverHub(hubId, fetcher)); }
      catch { pending++; }
    }
    return pending;
  }
}

// Invalidation carries no credential or key material; readers use origin-local IDB.
let invalidations: BroadcastChannel | undefined;
function channel(): BroadcastChannel | undefined {
  if (typeof BroadcastChannel === "undefined") return undefined;
  return invalidations ??= new BroadcastChannel("cassy-installation-generations");
}
export function notifyInstallation(hubId: string): void { try { channel()?.postMessage({ hubId }); } catch { /* IDB remains authoritative when browser policy denies notifications. */ } }
export function watchInstallations(change: (hubId: string) => void): () => void {
  const bus = channel();
  const listener = (event: MessageEvent) => { if (typeof event.data?.hubId === "string") change(event.data.hubId); };
  bus?.addEventListener("message", listener);
  return () => bus?.removeEventListener("message", listener);
}
