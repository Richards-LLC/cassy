import { pairingExchangeFailure, unreachableHubMessage } from "./pairing-messages";
import type { PendingInvitation, PairingRelayDelivery } from "./pending-pairing";
import type { PairingInstallIdentity, Scope, StoredMachine } from "./types";

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
  installationGeneration: number;
  stagePersisted: (machine: StoredMachine, identity: PairingInstallIdentity) => Promise<unknown>;
  activatePersisted: (identity: PairingInstallIdentity, signal?: AbortSignal) => Promise<boolean>;
  rollbackPersisted: (identity: PairingInstallIdentity) => Promise<boolean>;
  acknowledge?: (relay: PairingRelayDelivery, signal?: AbortSignal) => Promise<void>;
  signal?: AbortSignal;
  isCurrent?: () => boolean;
}

export class PairingExchangeError extends Error {
  /** The invitation is untouched, so the caller keeps it and lets the operator retry. */
  readonly recoverable: boolean;

  constructor(message = "This pairing invitation has expired or was already used.", options: { recoverable?: boolean } = {}) {
    super(message);
    this.name = "PairingExchangeError";
    this.recoverable = options.recoverable ?? false;
  }
}

/**
 * The hub accepted the exchange — the one-time invitation is consumed and the
 * device is recorded there — but this browser could not persist the credential.
 * Restoring storage does not un-consume the invitation, so this is its own
 * outcome: not "expired or already used", not a raw storage exception (F3).
 */
export class PairingStorageError extends PairingExchangeError {
  constructor(readonly cause: unknown) {
    super("The machine approved this browser, but this browser could not save access. Restore browser storage, then get a fresh invitation.");
    this.name = "PairingStorageError";
  }
}

export class PairingCleanupError extends PairingExchangeError {
  constructor(readonly cause: unknown) {
    super("Pairing cancellation is incomplete: the canceled credential is blocked, but durable cleanup is still pending.");
    this.name = "PairingCleanupError";
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
    throw new PairingExchangeError("This pairing invitation belongs to a different Cassy Commander origin.");
  }
  const baseUrl = invitation.hubUrl ?? (options.legacyHubUrl ? new URL(options.legacyHubUrl).origin : undefined);
  if (!baseUrl) {
    throw new PairingExchangeError("The pairing invitation does not identify a reachable hub. Enter the machine's hub URL and tap Pair again.", { recoverable: true });
  }
  // The invitation's declared scopes are a ceiling, not a suggestion. Requesting
  // above it is refused by the hub with an opaque 401, so the request is clamped
  // here and the form only ever narrows what the machine already granted.
  const granted = invitation.scopes ? [...invitation.scopes] : undefined;
  const scopes = granted
    ? (options.requestedScopes === undefined ? granted : options.requestedScopes.filter((scope) => granted.includes(scope)))
    : options.requestedScopes;
  if (!scopes?.length) {
    throw new PairingExchangeError("Tick at least one scope this invitation grants, then tap Pair again.", { recoverable: true });
  }
  const { privateKey, publicKey } = await options.createKey();
  ensureCurrent(options);
  const endpoint = new URL("/v1/auth/pairing/exchange", baseUrl);
  let response: Response;
  try {
    response = await options.fetcher(endpoint, {
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
  } catch (error) {
    // A cancelled exchange keeps its own cancellation path; anything else means
    // the request never reached the hub, so the invitation is still unused.
    if (options.signal?.aborted || options.isCurrent?.() === false) throw error;
    throw new PairingExchangeError(unreachableHubMessage(endpoint.origin), { recoverable: true });
  }
  ensureCurrent(options);
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    const failure = pairingExchangeFailure({ status: response.status, body: detail, controllerOrigin: options.controllerOrigin });
    throw new PairingExchangeError(failure.message, { recoverable: failure.keepInvitation });
  }
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
  const identity: PairingInstallIdentity = {
    machineId: machine.id,
    credentialId: machine.credentialId,
    generation: options.installationGeneration,
  };
  let staged = false;
  // Past this point the hub has consumed the invitation. A storage rejection is
  // reported as such; a cancellation or supersession keeps its own error.
  const persist = async <T>(step: () => Promise<T>): Promise<T> => {
    try {
      return await step();
    } catch (error) {
      if (error instanceof PairingExchangeError || options.signal?.aborted || options.isCurrent?.() === false) throw error;
      throw new PairingStorageError(error);
    }
  };
  try {
    ensureCurrent(options);
    await persist(() => options.stagePersisted(machine, identity));
    staged = true;
    ensureCurrent(options);
    if (invitation.relay && options.acknowledge) {
      await options.acknowledge(invitation.relay, options.signal).catch((error) => {
        ensureCurrent(options);
        void error;
      });
      ensureCurrent(options);
    }
    ensureCurrent(options);
    if (!await persist(() => options.activatePersisted(identity, options.signal))) {
      throw new PairingExchangeError("This pairing credential was superseded before installation completed.");
    }
    ensureCurrent(options);
  } catch (error) {
    if (staged) {
      try {
        await options.rollbackPersisted(identity);
      } catch (cleanupError) {
        throw new PairingCleanupError(cleanupError);
      }
    }
    throw error;
  }
  return machine;
}
