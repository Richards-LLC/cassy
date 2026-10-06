// Typed device client for the operator inbox cloud routes (wire v1, cas-9b7d).
//
// - Every bigint is a decimal string on the wire and stays one here; callers
//   compare with BigInt (`decimal()` validates the §4.2 pattern).
// - Browser calls use `credentials: "omit"`; no cookie or account API key is
//   ever attached. The proof binds the exact body bytes sent.
// - Unknown response keys are ignored (§13.2); an unknown error code is
//   classified by HTTP status (`retryable`).
// - A proof rejected as `pop_expired` with a server `Date` is re-signed once
//   with the observed skew, so a phone with a wrong clock still works.

import { popProof, type ProofBinding, type SigningKey } from "./pop";

export const WIRE_VERSION = 1;
export const DEFAULT_OPERATOR_ORIGIN = "https://petra-stella-cloud.vercel.app";
const REQUEST_DEADLINE_MS = 10_000;
const DECIMAL = /^(0|[1-9][0-9]{0,18})$/;

export type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

export class OperatorWireError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    readonly body: Record<string, unknown>,
    readonly retryAfterMs: number | null,
  ) {
    super(`${status} ${code}`);
    this.name = "OperatorWireError";
  }

  /** §13.2: 5xx and 429 retry with backoff; other 4xx do not, unless a known 409/410 recovery applies. */
  get retryable(): boolean {
    return this.status === 429 || this.status >= 500 || this.status === 0;
  }
}

export function decimal(value: unknown, field: string): string {
  if (typeof value !== "string" || !DECIMAL.test(value)) throw new OperatorWireError(0, "invalid_response", { field }, null);
  return value;
}

export function optionalDecimal(value: unknown, field: string): string | null {
  return value === null || value === undefined ? null : decimal(value, field);
}

export function str(value: unknown, field: string): string {
  if (typeof value !== "string") throw new OperatorWireError(0, "invalid_response", { field }, null);
  return value;
}

export function record(value: unknown, field: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new OperatorWireError(0, "invalid_response", { field }, null);
  }
  return value as Record<string, unknown>;
}

export function list(value: unknown, field: string): unknown[] {
  if (!Array.isArray(value)) throw new OperatorWireError(0, "invalid_response", { field }, null);
  return value;
}

/** Validate a configured cloud origin: HTTPS, or HTTP on IP loopback for non-production doubles. */
export function operatorOrigin(value: string | null | undefined): string {
  try {
    const parsed = new URL(value || DEFAULT_OPERATOR_ORIGIN);
    const loopback = parsed.hostname === "127.0.0.1" || parsed.hostname === "[::1]";
    if (parsed.username || parsed.password || parsed.pathname !== "/" || parsed.search || parsed.hash) throw new Error("not an origin");
    if (parsed.protocol !== "https:" && !(parsed.protocol === "http:" && loopback)) throw new Error("unsafe");
    return parsed.origin;
  } catch {
    return DEFAULT_OPERATOR_ORIGIN;
  }
}

export interface Grant {
  grantId: string;
  kind: string;
  accountId: string;
  origin: string | null;
  capabilities: string[];
  scopes: CommandScope[];
  generation: string;
  status: string;
  expiresAt: string;
}

export interface CommandScope {
  hub_id: string;
  project_id: string;
  session_id: string | null;
  operations: string[];
}

export function parseGrant(value: unknown): Grant {
  const grant = record(value, "grant");
  return {
    grantId: str(grant.grant_id, "grant_id"),
    kind: str(grant.kind, "kind"),
    accountId: str(grant.account_id, "account_id"),
    origin: typeof grant.origin === "string" ? grant.origin : null,
    capabilities: list(grant.capabilities ?? [], "capabilities").map((entry) => str(entry, "capability")),
    scopes: list(grant.scopes ?? [], "scopes").map((entry) => {
      const scope = record(entry, "scope");
      return {
        hub_id: str(scope.hub_id, "hub_id"),
        project_id: str(scope.project_id, "project_id"),
        session_id: typeof scope.session_id === "string" ? scope.session_id : null,
        operations: list(scope.operations ?? [], "operations").map((op) => str(op, "operation")),
      };
    }),
    generation: decimal(grant.grant_generation, "grant_generation"),
    status: str(grant.status, "status"),
    expiresAt: str(grant.expires_at, "expires_at"),
  };
}

export interface DeviceCredential {
  key: SigningKey;
  grantId: string;
  generation: string;
}

export interface OperatorClientOptions {
  origin: string;
  fetch?: Fetcher;
  /** Wall clock in ms; skew learned from server `Date` headers is added. */
  now?: () => number;
}

export interface RequestOptions {
  method: "GET" | "POST" | "PUT" | "DELETE";
  path: string;
  body?: unknown;
  auth: "none" | "grant" | { enrollmentId: string; key: SigningKey };
  signal?: AbortSignal;
}

export class OperatorClient {
  readonly origin: string;
  readonly audience: string;
  private readonly fetcher: Fetcher;
  private readonly now: () => number;
  private skewMs = 0;
  credential: DeviceCredential | null = null;

  constructor(options: OperatorClientOptions) {
    this.origin = operatorOrigin(options.origin);
    this.audience = `${this.origin}/api/operator`;
    this.fetcher = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
    this.now = options.now ?? Date.now;
  }

  private binding(auth: RequestOptions["auth"]): { key: SigningKey; binding: ProofBinding } | null {
    if (auth === "none") return null;
    if (auth === "grant") {
      if (!this.credential) throw new OperatorWireError(401, "not_enrolled", {}, null);
      return {
        key: this.credential.key,
        binding: { kind: "grant", grantId: this.credential.grantId, generation: this.credential.generation },
      };
    }
    return { key: auth.key, binding: { kind: "enrollment", enrollmentId: auth.enrollmentId } };
  }

  private async send(options: RequestOptions, retriedSkew: boolean): Promise<Record<string, unknown>> {
    const bytes = options.body === undefined ? new Uint8Array() : new TextEncoder().encode(JSON.stringify(options.body));
    const headers: Record<string, string> = {};
    if (bytes.length > 0) headers["Content-Type"] = "application/json";
    const signer = this.binding(options.auth);
    if (signer) {
      headers["PSC-PoP-Proof"] = await popProof(signer.key, {
        audience: this.audience,
        method: options.method,
        pathAndQuery: options.path,
        body: bytes,
        binding: signer.binding,
        now: this.now() + this.skewMs,
      });
      if (signer.binding.kind === "grant") headers.Authorization = `PSC-PoP ${signer.binding.grantId}`;
    }
    const deadline = AbortSignal.timeout(REQUEST_DEADLINE_MS);
    const signal = options.signal ? anySignal([options.signal, deadline]) : deadline;
    let response: Response;
    try {
      response = await this.fetcher(`${this.origin}${options.path}`, {
        method: options.method,
        headers,
        body: bytes.length > 0 ? bytes : undefined,
        credentials: "omit",
        cache: "no-store",
        referrerPolicy: "no-referrer",
        signal,
      });
    } catch (error) {
      if (options.signal?.aborted) throw error;
      throw new OperatorWireError(0, deadline.aborted ? "request_timeout" : "network_unavailable", {}, null);
    }
    let parsed: Record<string, unknown> = {};
    const text = await response.text().catch(() => "");
    if (text) {
      try {
        parsed = record(JSON.parse(text), "body");
      } catch {
        parsed = {};
      }
    }
    if (response.ok) return parsed;
    const code = typeof parsed.error === "string" ? parsed.error : `http_${response.status}`;
    if (code === "pop_expired" && !retriedSkew) {
      const serverDate = Date.parse(response.headers.get("Date") ?? "");
      if (Number.isFinite(serverDate)) {
        this.skewMs = serverDate - this.now();
        return this.send(options, true);
      }
    }
    const retryAfter = Number(response.headers.get("Retry-After"));
    throw new OperatorWireError(response.status, code, parsed, Number.isFinite(retryAfter) && retryAfter >= 0 ? retryAfter * 1000 : null);
  }

  request(options: RequestOptions): Promise<Record<string, unknown>> {
    return this.send(options, false);
  }

  // ------------------------------------------------------------ enrollment (§5.1, §5.3)

  createEnrollment(body: {
    origin: string;
    device_label: string;
    signing_jwk: SigningKey["publicJwk"];
    encryption_public_key: string;
    email?: string;
  }) {
    return this.request({
      method: "POST",
      path: "/api/operator/enrollments",
      body: { wire_version: WIRE_VERSION, kind: "device", ...body },
      auth: "none",
    });
  }

  pollEnrollment(enrollmentId: string, pollSecret: string) {
    return this.request({
      method: "POST",
      path: `/api/operator/enrollments/${encodeURIComponent(enrollmentId)}/poll`,
      body: { wire_version: WIRE_VERSION, poll_secret: pollSecret },
      auth: "none",
    });
  }

  completeEnrollment(enrollmentId: string, key: SigningKey, pollSecret: string, encryptionKeyCheck: string) {
    return this.request({
      method: "POST",
      path: `/api/operator/enrollments/${encodeURIComponent(enrollmentId)}/complete`,
      body: { wire_version: WIRE_VERSION, poll_secret: pollSecret, encryption_key_check: encryptionKeyCheck },
      auth: { enrollmentId, key },
    });
  }

  // ------------------------------------------------------------ grant (§4.1)

  grantsMe() {
    return this.request({ method: "GET", path: "/api/operator/grants/me", auth: "grant" });
  }

  renewGrant() {
    return this.request({ method: "POST", path: "/api/operator/grants/renew", body: { wire_version: WIRE_VERSION }, auth: "grant" });
  }

  signOut() {
    return this.request({ method: "DELETE", path: "/api/operator/devices/me", auth: "grant" });
  }

  principals() {
    return this.request({ method: "GET", path: "/api/operator/principals", auth: "grant" });
  }

  // ------------------------------------------------------------ keys (§6.4)

  keyWraps() {
    return this.request({ method: "GET", path: "/api/operator/keys/wraps", auth: "grant" });
  }

  // ------------------------------------------------------------ feed (§8)

  replay(generation: string, after: string, limit = 200, signal?: AbortSignal) {
    const query = `generation=${encodeURIComponent(generation)}&after=${encodeURIComponent(after)}&limit=${limit}`;
    return this.request({ method: "GET", path: `/api/operator/feed?${query}`, auth: "grant", signal });
  }

  ackPersisted(generation: string, acks: { event_id: string; digest: string }[]) {
    return this.request({
      method: "POST",
      path: "/api/operator/feed/acks",
      body: { wire_version: WIRE_VERSION, feed_generation: generation, acks },
      auth: "grant",
    });
  }

  getCursor() {
    return this.request({ method: "GET", path: "/api/operator/devices/me/cursor", auth: "grant" });
  }

  putCursor(generation: string, cursor: string, acceptedExpiredThrough: string | null) {
    return this.request({
      method: "PUT",
      path: "/api/operator/devices/me/cursor",
      body: {
        wire_version: WIRE_VERSION,
        feed_generation: generation,
        cursor,
        ...(acceptedExpiredThrough === null ? {} : { accepted_expired_through: acceptedExpiredThrough }),
      },
      auth: "grant",
    });
  }

  putReadMark(generation: string, mark: { hub_id: string; project_id: string; session_id: string; sequence: string }) {
    return this.request({
      method: "PUT",
      path: "/api/operator/read-marks",
      body: { wire_version: WIRE_VERSION, feed_generation: generation, ...mark },
      auth: "grant",
    });
  }

  getReadMarks(generation: string, updatedAfter: string | null, cursor: string | null) {
    const parts = [`feed_generation=${encodeURIComponent(generation)}`];
    if (updatedAfter) parts.push(`updated_after=${encodeURIComponent(updatedAfter)}`);
    if (cursor) parts.push(`cursor=${encodeURIComponent(cursor)}`);
    return this.request({ method: "GET", path: `/api/operator/read-marks?${parts.join("&")}`, auth: "grant" });
  }

  // ------------------------------------------------------------ commands (§10)

  submitCommand(body: Record<string, unknown>) {
    return this.request({
      method: "POST",
      path: "/api/operator/commands",
      body: { wire_version: WIRE_VERSION, ...body },
      auth: "grant",
    });
  }

  commandStatus(commandId: string) {
    return this.request({ method: "GET", path: `/api/operator/commands/${encodeURIComponent(commandId)}`, auth: "grant" });
  }

  cancelCommand(commandId: string) {
    return this.request({
      method: "POST",
      path: `/api/operator/commands/${encodeURIComponent(commandId)}/cancel`,
      body: { wire_version: WIRE_VERSION },
      auth: "grant",
    });
  }
}

function anySignal(signals: AbortSignal[]): AbortSignal {
  const any = (AbortSignal as unknown as { any?: (signals: AbortSignal[]) => AbortSignal }).any;
  if (any) return any(signals);
  const controller = new AbortController();
  for (const signal of signals) {
    if (signal.aborted) {
      controller.abort(signal.reason);
      break;
    }
    signal.addEventListener("abort", () => controller.abort(signal.reason), { once: true });
  }
  return controller.signal;
}
