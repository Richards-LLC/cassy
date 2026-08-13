import type { PendingInvitation, PairingRelayDelivery } from "./pending-pairing";
import type { Scope, StoredMachine } from "./types";

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

interface ExchangeOptions {
  invitation: PendingInvitation;
  controllerOrigin: string;
  legacyHubUrl?: string;
  machineLabel?: string;
  deviceLabel: string;
  operatorLabel: string;
  requestedScopes?: Scope[];
  fetcher: Fetcher;
  createKey: () => Promise<{ privateKey: CryptoKey; publicKey: JsonWebKey }>;
  persist: (machine: StoredMachine) => Promise<unknown>;
  removePersisted?: (machineId: string) => Promise<unknown>;
  acknowledge?: (relay: PairingRelayDelivery, signal?: AbortSignal) => Promise<void>;
  signal?: AbortSignal;
  isCurrent?: () => boolean;
}

export class PairingExchangeError extends Error {
  constructor(message = "This pairing invitation has expired or was already used.") {
    super(message);
    this.name = "PairingExchangeError";
  }
}

function ensureCurrent(options: ExchangeOptions): void {
  if (options.signal?.aborted || options.isCurrent?.() === false) {
    throw new PairingExchangeError("Pairing was cancelled before the credential could be installed.");
  }
}

export async function exchangePendingPairing(options: ExchangeOptions): Promise<StoredMachine> {
  const { invitation } = options;
  if (invitation.controllerOrigin && invitation.controllerOrigin !== options.controllerOrigin) {
    throw new PairingExchangeError("This pairing invitation belongs to a different Commander origin.");
  }
  const baseUrl = invitation.hubUrl ?? (options.legacyHubUrl ? new URL(options.legacyHubUrl).origin : undefined);
  if (!baseUrl) throw new PairingExchangeError("The pairing invitation does not identify a reachable hub.");
  const scopes = invitation.scopes ? [...invitation.scopes] : options.requestedScopes;
  if (!scopes?.length) throw new PairingExchangeError("Choose at least one read-only scope.");
  const { privateKey, publicKey } = await options.createKey();
  ensureCurrent(options);
  const response = await options.fetcher(new URL("/v1/auth/pairing/exchange", baseUrl), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "omit",
    signal: options.signal,
    body: JSON.stringify({
      token: invitation.token,
      hub_id: invitation.hubId,
      controller_origin: options.controllerOrigin,
      public_key_jwk: publicKey,
      device_label: options.deviceLabel,
      operator_label: options.operatorLabel,
      requested_scopes: scopes,
    }),
  });
  ensureCurrent(options);
  if (!response.ok) throw new PairingExchangeError();
  const credential = await response.json().catch(() => null) as Record<string, unknown> | null;
  ensureCurrent(options);
  if (!credential || typeof credential.device_id !== "string" || typeof credential.credential_id !== "string" || typeof credential.credential !== "string" || typeof credential.expires_at !== "string" || !Array.isArray(credential.scopes)) {
    throw new PairingExchangeError("The paired hub returned an invalid credential.");
  }
  const credentialScopes = credential.scopes as unknown[];
  if (!credentialScopes.length || new Set(credentialScopes).size !== credentialScopes.length || credentialScopes.some((scope) => typeof scope !== "string" || !scopes.includes(scope as Scope))) {
    throw new PairingExchangeError("The paired hub returned invalid credential scopes.");
  }
  const machine: StoredMachine = {
    id: invitation.hubId,
    label: invitation.machineLabel ?? options.machineLabel ?? invitation.hubId.slice(0, 8),
    baseUrl,
    deviceId: credential.device_id,
    credentialId: credential.credential_id,
    credential: credential.credential,
    expiresAt: credential.expires_at,
    scopes: credentialScopes as Scope[],
    publicKey,
    privateKey,
  };
  let persisted = false;
  try {
    ensureCurrent(options);
    await options.persist(machine);
    persisted = true;
    ensureCurrent(options);
    if (invitation.relay && options.acknowledge) {
      await options.acknowledge(invitation.relay, options.signal).catch((error) => {
        ensureCurrent(options);
        void error;
      });
      ensureCurrent(options);
    }
  } catch (error) {
    if (persisted && (options.signal?.aborted || options.isCurrent?.() === false) && options.removePersisted) {
      await options.removePersisted(machine.id).catch(() => undefined);
    }
    throw error;
  }
  return machine;
}
