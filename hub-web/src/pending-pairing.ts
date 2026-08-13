import type { Scope } from "./types";

const STORAGE_KEY = "cas.commander.pending-pairing.v1";

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
  constructor(private readonly storage: Storage, private readonly now: () => number = Date.now) {}

  load(): PendingPairing | null {
    const raw = this.storage.getItem(STORAGE_KEY);
    if (!raw) return null;
    try {
      const value: unknown = JSON.parse(raw);
      if (!isPendingPairing(value)) throw new Error("invalid pending pairing");
      if (value.expiresAt && (Number.isNaN(Date.parse(value.expiresAt)) || Date.parse(value.expiresAt) <= this.now())) {
        this.clear();
        return null;
      }
      return value;
    } catch {
      this.clear();
      return null;
    }
  }

  save(value: PendingPairing): void {
    this.storage.setItem(STORAGE_KEY, JSON.stringify(value));
  }

  saveLegacy(token: string, hubId: string): PendingInvitation {
    const invitation: PendingInvitation = { kind: "invitation", token, hubId };
    this.save(invitation);
    return invitation;
  }

  clear(): void {
    this.storage.removeItem(STORAGE_KEY);
  }
}
