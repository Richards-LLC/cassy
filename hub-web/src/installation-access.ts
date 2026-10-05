import { createDeviceKey } from "./dpop";
import { exchangePendingPairing, PairingCleanupError, PairingExchangeError, type ExchangeOptions } from "./pairing-exchange";
import type { MachineCatalog } from "./storage";
import type { PairingInstallIdentity, StoredMachine, Scope } from "./types";

export type AccountEnrollment = { state: "unenrolled" };
export type InstallationKey = Awaited<ReturnType<typeof createDeviceKey>>;
export interface InstallationRecord extends InstallationKey {
  id: string;
  pending?: { operationId: string; baseUrl: string; controllerOrigin: string; identity?: PairingInstallIdentity };
}
export interface InstallationStore {
  get(id: string): Promise<InstallationRecord | undefined>;
  put(record: InstallationRecord): Promise<unknown>;
  list(): Promise<InstallationRecord[]>;
}
export type InstallationLock = <T>(hubId: string, run: () => Promise<T>, signal?: AbortSignal) => Promise<T>;
export const installationLock: InstallationLock = (hubId, run, signal) => {
  if (!navigator.locks) return Promise.reject(new Error("This browser cannot safely coordinate pairing across tabs. Use a browser with Web Locks support."));
  return navigator.locks.request(`cassy-installation:${hubId}`, { mode: "exclusive", ...(signal ? { signal } : {}) }, run);
};
const encoder = new TextEncoder();
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
export async function installationTranscript(options: ExchangeOptions, record: InstallationRecord, machine: StoredMachine | undefined, operationId: string, credential: string, scopes: Scope[]): Promise<unknown[]> {
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

  async pair(options: ExchangeOptions): Promise<StoredMachine> {
    return this.lock(options.invitation.hubId, async () => {
      await this.recoverHub(options.invitation.hubId, options.fetcher);
      const baseUrl = options.invitation.hubUrl ?? new URL(options.legacyHubUrl!).origin;
      const id = `${options.invitation.hubId}@${new URL(baseUrl).origin}`;
      const prior = (await this.catalog.snapshot()).machines.find((m) => m.id === options.invitation.hubId && new URL(m.baseUrl).origin === new URL(baseUrl).origin);
      let record = await this.store.get(id);
      if (!record) {
        record = { id, ...(prior ? { privateKey: prior.privateKey, publicKey: prior.publicKey } : await this.createKey()) };
        await this.store.put(record);
      }
      const operationId = crypto.randomUUID();
      const credential = b64url(crypto.getRandomValues(new Uint8Array(32)));
      const ceiling = options.invitation.scopes ?? options.requestedScopes ?? [];
      const scopes = options.requestedScopes ? options.requestedScopes.filter((scope) => ceiling.includes(scope)) : ceiling;
      const transcript = await installationTranscript(options, record, prior, operationId, credential, scopes);
      const proof = await signInstallation(record.privateKey, transcript);
      record.pending = { operationId, baseUrl, controllerOrigin: options.controllerOrigin };
      await this.store.put(record); // Before POST: recovery can cancel even a lost response.
      const current = record;
      try {
        const machine = await exchangePendingPairing({ ...options,
          createKey: async () => current,
          installation: { operation_id: operationId, credential, device_id: prior?.deviceId ?? null,
            expected_generation: prior?.credentialGeneration ?? 0, proof, previous_proof: null },
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
        delete current.pending;
        await this.store.put(current);
        return machine;
      } catch (error) {
        // This also covers cancellation before response decoding or before catalog staging.
        try {
          await this.action(current, "abort", options.fetcher, AbortSignal.timeout(10_000));
          if (current.pending?.identity) await this.catalog.rollback(current.pending.identity);
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
    const proof = await signInstallation(record.privateKey, [`cassy-installation-${action}-v1`, record.id.split("@")[0], pending.controllerOrigin, pending.operationId]);
    const response = await fetcher(new URL(`/v1/auth/pairing/${action}`, pending.baseUrl), {
      method: "POST", credentials: "omit", headers: { "Content-Type": "application/json" }, signal,
      body: JSON.stringify({ operation_id: pending.operationId, controller_origin: pending.controllerOrigin, public_key_jwk: record.publicKey, proof }),
    });
    if (!response.ok) throw new Error("Installation recovery needs the hub to confirm its credential generation. Retry cleanup before pairing again.");
  }

  private async recoverHub(hubId: string, fetcher: ExchangeOptions["fetcher"]): Promise<void> {
    for (const record of await this.store.list()) {
      if (!record.pending || record.id.split("@")[0] !== hubId) continue;
      await this.action(record, "abort", fetcher, AbortSignal.timeout(10_000));
      if (record.pending.identity) await this.catalog.rollback(record.pending.identity);
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
