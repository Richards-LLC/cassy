import type { Scope } from "./types";

const STORAGE_KEY = "cas.commander.pending-pairing.v1";
const CLEARED_TOMBSTONE = '{"kind":"cleared"}';

export interface PendingRelayRequest {
  kind: "relay-request";
  pairingRequestId: string;
  userCode: string;
  pollSecret: string;
  controllerOrigin: string;
  requestedScopes: Scope[];
  expiresAt: string;
  interval: number;
}

export interface PairingRelayDelivery {
  pairingRequestId: string;
  pollSecret: string;
  deliveryId: string;
}

export interface PendingInvitation {
  kind: "invitation";
  token: string;
  hubId: string;
  hubUrl?: string;
  machineLabel?: string;
  controllerOrigin?: string;
  scopes?: readonly Scope[];
  expiresAt?: string;
  relay?: PairingRelayDelivery;
}

export type PendingPairing = PendingRelayRequest | PendingInvitation;

type PairingStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export type PendingPairingLoadOutcome =
  | { readonly kind: "none" }
  | { readonly kind: "expired" }
  | { readonly kind: "pending"; readonly value: PendingPairing };

export interface PendingPairingClearResult {
  /** `removeItem` was denied, so a tombstone rather than deletion was persisted. */
  persistentRemovalFailed: boolean;
  /** Reload cannot resume the cancelled request from the persistent store. */
  failClosed: boolean;
}

class MemoryPairingStorage implements PairingStorage {
  private readonly values = new Map<string, string>();
  getItem(key: string): string | null { return this.values.get(key) ?? null; }
  setItem(key: string, value: string): void { this.values.set(key, value); }
  removeItem(key: string): void { this.values.delete(key); }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.length > 0 && value.every((item) => typeof item === "string");
}

function isPendingPairing(value: unknown): value is PendingPairing {
  if (!isRecord(value)) return false;
  if (value.kind === "relay-request") {
    return [value.pairingRequestId, value.userCode, value.pollSecret, value.controllerOrigin, value.expiresAt].every((field) => typeof field === "string" && field.length > 0)
      && isStringArray(value.requestedScopes)
      && typeof value.interval === "number" && Number.isFinite(value.interval) && value.interval > 0;
  }
  if (value.kind !== "invitation") return false;
  if (typeof value.token !== "string" || !value.token || typeof value.hubId !== "string" || !value.hubId) return false;
  if (value.expiresAt !== undefined && typeof value.expiresAt !== "string") return false;
  if (value.scopes !== undefined && !isStringArray(value.scopes)) return false;
  if (value.relay !== undefined) {
    if (!isRecord(value.relay)) return false;
    if (![value.relay.pairingRequestId, value.relay.pollSecret, value.relay.deliveryId].every((field) => typeof field === "string" && field.length > 0)) return false;
  }
  return true;
}

export class PendingPairingStore {
  private readonly fallback = new MemoryPairingStorage();

  constructor(private readonly storage?: PairingStorage, private readonly now: () => number = Date.now) {}

  load(): PendingPairing | null {
    const outcome = this.loadOutcome();
    return outcome.kind === "pending" ? outcome.value : null;
  }

  /**
   * Like `load()`, but says why nothing came back. An expired invitation is
   * cleared exactly as before; the outcome carries no token, only the fact
   * that the operator's link ran out before it was used (F6).
   */
  loadOutcome(): PendingPairingLoadOutcome {
    let raw: string | null;
    try {
      raw = this.storage?.getItem(STORAGE_KEY) ?? this.fallback.getItem(STORAGE_KEY);
    } catch {
      raw = this.fallback.getItem(STORAGE_KEY);
    }
    if (!raw) return { kind: "none" };
    if (raw === CLEARED_TOMBSTONE) return { kind: "none" };
    try {
      const value: unknown = JSON.parse(raw);
      if (!isPendingPairing(value)) throw new Error("invalid pending pairing");
      if (value.expiresAt && (Number.isNaN(Date.parse(value.expiresAt)) || Date.parse(value.expiresAt) <= this.now())) {
        this.clear();
        return { kind: "expired" };
      }
      return { kind: "pending", value };
    } catch {
      this.clear();
      return { kind: "none" };
    }
  }

  save(value: PendingPairing): void {
    const serialized = JSON.stringify(value);
    this.fallback.setItem(STORAGE_KEY, serialized);
    try { this.storage?.setItem(STORAGE_KEY, serialized); } catch { /* private storage can be denied */ }
  }

  saveLegacy(token: string, hubId: string, scopes?: readonly Scope[]): PendingInvitation {
    const invitation: PendingInvitation = { kind: "invitation", token, hubId, ...(scopes?.length ? { scopes: [...scopes] } : {}) };
    this.save(invitation);
    return invitation;
  }

  clear(): PendingPairingClearResult {
    this.fallback.removeItem(STORAGE_KEY);
    try {
      this.storage?.removeItem(STORAGE_KEY);
      return { persistentRemovalFailed: false, failClosed: true };
    } catch {
      try {
        this.storage?.setItem(STORAGE_KEY, CLEARED_TOMBSTONE);
        return { persistentRemovalFailed: true, failClosed: true };
      } catch {
        // Keep the current page clear and let the caller surface that reload safety is unknown.
        return { persistentRemovalFailed: true, failClosed: false };
      }
    }
  }
}

/** Acquire browser storage without allowing a denied getter to stop fragment scrubbing or boot. */
export function pendingPairingStoreFor(provider: { readonly sessionStorage: Storage }): PendingPairingStore {
  try {
    return new PendingPairingStore(provider.sessionStorage);
  } catch {
    return new PendingPairingStore();
  }
}
