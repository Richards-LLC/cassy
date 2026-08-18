import type { PairingRelayDelivery, PendingInvitation, PendingRelayRequest } from "./pending-pairing";
import type { Scope } from "./types";

// The relay may only request these Cassy Commander scopes. The machine-side
// `cas hub authorize` prompt is the consent boundary: it displays the origin
// and requested/granted scopes and may narrow, but never elevate, this set.
export const DEFAULT_PAIRING_SCOPES: Scope[] = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];
const USER_CODE = /^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/;
const CREATE_PATH = "/api/hub/pairing/requests";
const POLL_PATH = "/api/hub/pairing/requests/poll";
const ACK_PATH = "/api/hub/pairing/requests/acknowledge";

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

export class PairingRelayError extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
    this.name = "PairingRelayError";
  }
}

/**
 * Validate the reviewed, static-bundle relay boundary. Relay metadata is an
 * HTTPS origin only: paths, credentials, query strings, and fragments could
 * redirect pairing capabilities to an unreviewed endpoint and fail closed.
 */
export function pairingRelayOrigin(value: string | null): string | null {
  if (!value) return null;
  try {
    const parsed = new URL(value);
    if (parsed.protocol !== "https:" || parsed.username || parsed.password || parsed.pathname !== "/" || parsed.search || parsed.hash) return null;
    return parsed.origin;
  } catch {
    return null;
  }
}

function relayEndpoint(origin: string, path: string): URL {
  const canonical = pairingRelayOrigin(origin);
  if (!canonical) throw new PairingRelayError("relay_unavailable", "Page-initiated pairing is unavailable in this deployment.");
  return new URL(path, canonical);
}

/** Normalize a safe hub root URL to the origin used by authenticated traffic. */
export function normalizeHubOrigin(value: string): string {
  try {
    const parsed = new URL(value);
    if (parsed.username || parsed.password || parsed.pathname !== "/" || parsed.search || parsed.hash) throw new Error("not a root URL");
    const loopback = parsed.hostname === "[::1]" || parsed.hostname === "::1" || /^127(?:\.\d{1,3}){3}$/.test(parsed.hostname);
    if (parsed.protocol !== "https:" && !(parsed.protocol === "http:" && loopback)) throw new Error("unsafe origin");
    return parsed.origin;
  } catch {
    throw new PairingRelayError("invalid_response", "The paired machine returned an invalid hub origin.");
  }
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null ? value as Record<string, unknown> : null;
}

async function jsonRecord(response: Response): Promise<Record<string, unknown>> {
  const value: unknown = await response.json().catch(() => null);
  const parsed = record(value);
  if (!parsed) throw new PairingRelayError("invalid_response", "The pairing service returned an invalid response.");
  return parsed;
}

function stringField(value: Record<string, unknown>, name: string): string {
  const field = value[name];
  if (typeof field !== "string" || !field) throw new PairingRelayError("invalid_response", "The pairing service returned an invalid response.");
  return field;
}

function scopeFields(value: Record<string, unknown>, name: string): Scope[] {
  const field = value[name];
  if (!Array.isArray(field) || field.length === 0 || new Set(field).size !== field.length || field.some((scope) => !DEFAULT_PAIRING_SCOPES.includes(scope as Scope))) {
    throw new PairingRelayError("invalid_response", "The pairing service returned invalid pairing scopes.");
  }
  return field as Scope[];
}

function expiryField(value: Record<string, unknown>, name = "expires_at"): string {
  const expiry = stringField(value, name);
  if (Number.isNaN(Date.parse(expiry))) throw new PairingRelayError("invalid_response", "The pairing service returned an invalid expiry.");
  return expiry;
}

function intervalField(value: Record<string, unknown>): number {
  const interval = value.interval;
  if (typeof interval !== "number" || !Number.isFinite(interval) || interval <= 0) {
    throw new PairingRelayError("invalid_response", "The pairing service returned an invalid poll interval.");
  }
  return interval;
}

function relayError(status: number, code: unknown): PairingRelayError {
  if (status === 403 && code === "request_mismatch") return new PairingRelayError("request_mismatch", "This pairing request no longer matches this page.");
  if (status === 410 && code === "expired_request") return new PairingRelayError("expired_request", "This pairing request has expired.");
  return new PairingRelayError(typeof code === "string" ? code : "relay_error", "The pairing service refused the request.");
}

export async function createPairingRequest(fetcher: Fetcher, relayOrigin: string, controllerOrigin: string, requestedScopes: Scope[] = DEFAULT_PAIRING_SCOPES, email?: string, signal?: AbortSignal): Promise<PendingRelayRequest> {
  if (!requestedScopes.length || new Set(requestedScopes).size !== requestedScopes.length || requestedScopes.some((scope) => !DEFAULT_PAIRING_SCOPES.includes(scope))) {
    throw new PairingRelayError("unsupported_scope", "Page-initiated pairing requested an unsupported Cassy Commander scope.");
  }
  const body: Record<string, unknown> = { wire_version: 1, controller_origin: controllerOrigin, requested_scopes: requestedScopes };
  if (email) body.email = email;
  const response = await fetcher(relayEndpoint(relayOrigin, CREATE_PATH), {
    method: "POST", headers: { "Content-Type": "application/json" }, credentials: "omit", body: JSON.stringify(body), signal,
  });
  const value = await jsonRecord(response);
  if (response.status !== 201) throw relayError(response.status, value.error);
  if (value.wire_version !== 1) throw new PairingRelayError("invalid_response", "The pairing service returned an unsupported wire version.");
  if (value.expires_in !== 600) throw new PairingRelayError("invalid_response", "The pairing service returned an invalid request lifetime.");
  const returnedOrigin = stringField(value, "controller_origin");
  if (returnedOrigin !== controllerOrigin) throw new PairingRelayError("origin_mismatch", "The pairing service returned a different controller origin.");
  const userCode = stringField(value, "user_code");
  if (!USER_CODE.test(userCode)) throw new PairingRelayError("invalid_response", "The pairing service returned an invalid pairing code.");
  const scopes = scopeFields(value, "requested_scopes");
  if (scopes.length !== requestedScopes.length || scopes.some((scope) => !requestedScopes.includes(scope))) {
    throw new PairingRelayError("scope_mismatch", "The pairing service returned different pairing scopes.");
  }
  const expiresAt = expiryField(value);
  return {
    kind: "relay-request",
    pairingRequestId: stringField(value, "pairing_request_id"),
    userCode,
    pollSecret: stringField(value, "poll_secret"),
    controllerOrigin: returnedOrigin,
    requestedScopes: scopes,
    expiresAt,
    interval: intervalField(value),
  };
}

export type PollResult =
  | { kind: "pending" | "claimed"; interval: number; expiresAt: string }
  | { kind: "slow-down"; interval: number }
  | { kind: "authorized"; invitation: PendingInvitation };

export async function pollPairingRequest(fetcher: Fetcher, relayOrigin: string, request: PendingRelayRequest, signal?: AbortSignal): Promise<PollResult> {
  const response = await fetcher(relayEndpoint(relayOrigin, POLL_PATH), {
    method: "POST", headers: { "Content-Type": "application/json" }, credentials: "omit", signal,
    body: JSON.stringify({ wire_version: 1, pairing_request_id: request.pairingRequestId, poll_secret: request.pollSecret }),
  });
  const value = await jsonRecord(response);
  if (response.status === 429 && value.error === "slow_down") {
    const retryAfter = Number(response.headers.get("Retry-After"));
    return { kind: "slow-down", interval: Number.isFinite(retryAfter) && retryAfter > 0 ? Math.max(request.interval, retryAfter) : request.interval + 1 };
  }
  if (response.status === 202) {
    if (value.wire_version !== 1 || (value.status !== "authorization_pending" && value.status !== "machine_claimed")) {
      throw new PairingRelayError("invalid_response", "The pairing service returned an invalid pending status.");
    }
    return { kind: value.status === "machine_claimed" ? "claimed" : "pending", interval: intervalField(value), expiresAt: expiryField(value) };
  }
  if (response.status !== 200) throw relayError(response.status, value.error);
  if (value.wire_version !== 1 || value.status !== "authorized") throw new PairingRelayError("invalid_response", "The pairing service returned an invalid authorization status.");
  const invitation = record(value.invitation);
  if (!invitation) throw new PairingRelayError("invalid_response", "The pairing service returned an invalid invitation.");
  const controllerOrigin = stringField(invitation, "controller_origin");
  if (controllerOrigin !== request.controllerOrigin) throw new PairingRelayError("origin_mismatch", "The paired machine returned a different controller origin.");
  const hubOrigin = normalizeHubOrigin(stringField(invitation, "hub_url"));
  const scopes = scopeFields(invitation, "scopes");
  if (scopes.some((scope) => !request.requestedScopes.includes(scope))) throw new PairingRelayError("scope_escalation", "The paired machine returned elevated scopes.");
  return {
    kind: "authorized",
    invitation: {
      kind: "invitation",
      token: stringField(invitation, "token"),
      hubId: stringField(invitation, "hub_id"),
      hubUrl: hubOrigin,
      machineLabel: stringField(invitation, "machine_label"),
      controllerOrigin,
      scopes,
      expiresAt: expiryField(invitation),
      relay: { pairingRequestId: request.pairingRequestId, pollSecret: request.pollSecret, deliveryId: stringField(value, "delivery_id") },
    },
  };
}

export async function acknowledgePairing(fetcher: Fetcher, relayOrigin: string, relay: PairingRelayDelivery, signal?: AbortSignal): Promise<void> {
  const response = await fetcher(relayEndpoint(relayOrigin, ACK_PATH), {
    method: "POST", headers: { "Content-Type": "application/json" }, credentials: "omit", signal,
    body: JSON.stringify({ wire_version: 1, pairing_request_id: relay.pairingRequestId, poll_secret: relay.pollSecret, delivery_id: relay.deliveryId }),
  });
  if (response.status !== 204) throw new PairingRelayError("acknowledgement_failed", "The pairing service could not acknowledge delivery.");
}
