// Cloud issuer tokens (contract §4.6): verify a compact ES256 JWS against the
// issuer JWKS, with an exact `typ` per token kind so no token stands in for
// another. The JWKS is cached for at most one hour (Q3 default) and refetched
// once when a `kid` is unknown, which covers a rotation published since the
// last fetch. Verify-only keys verify; the cloud keeps them 91 days.

import { b64urlDecode } from "./hpke";

export const ISSUER_TYP = {
  epochManifest: "psc-op-epoch-manifest+jwt",
  machineBinding: "psc-op-machine-binding+jwt",
  enrollment: "psc-op-enrollment+jwt",
  commandAdmission: "psc-op-command-admission+jwt",
  machineObservation: "psc-op-machine-observation+jwt",
} as const;

export type IssuerTyp = (typeof ISSUER_TYP)[keyof typeof ISSUER_TYP];

export const JWKS_MAX_AGE_MS = 60 * 60 * 1000;
/** An unknown `kid` forces at most one refetch per minute. */
const UNKNOWN_KID_REFETCH_MS = 60 * 1000;

export type IssuerFailure = "malformed" | "wrong_type" | "unknown_key" | "bad_signature" | "expired" | "jwks_unavailable";

export class IssuerTokenError extends Error {
  constructor(readonly reason: IssuerFailure, detail: string) {
    super(`${reason}: ${detail}`);
    this.name = "IssuerTokenError";
  }
}

interface IssuerJwk {
  kty: "EC";
  crv: "P-256";
  x: string;
  y: string;
  kid: string;
}

export type JwksFetcher = () => Promise<unknown>;

const decoder = new TextDecoder("utf-8", { fatal: true });
const encoder = new TextEncoder();

function parseSegment(segment: string, what: string): Record<string, unknown> {
  let value: unknown;
  try {
    value = JSON.parse(decoder.decode(b64urlDecode(segment, what)));
  } catch {
    throw new IssuerTokenError("malformed", what);
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new IssuerTokenError("malformed", what);
  return value as Record<string, unknown>;
}

function parseJwks(value: unknown): Map<string, IssuerJwk> {
  const keys = (value as { keys?: unknown } | null)?.keys;
  if (!Array.isArray(keys)) throw new IssuerTokenError("jwks_unavailable", "keys");
  const out = new Map<string, IssuerJwk>();
  for (const key of keys) {
    const k = key as Record<string, unknown>;
    if (k.kty !== "EC" || k.crv !== "P-256" || typeof k.x !== "string" || typeof k.y !== "string" || typeof k.kid !== "string") continue;
    if (k.alg !== undefined && k.alg !== "ES256") continue;
    if (k.use !== undefined && k.use !== "sig") continue;
    out.set(k.kid, { kty: "EC", crv: "P-256", x: k.x, y: k.y, kid: k.kid });
  }
  return out;
}

export interface VerifiedToken {
  header: Record<string, unknown>;
  claims: Record<string, unknown>;
}

export class IssuerKeys {
  private keys: Map<string, IssuerJwk> | null = null;
  private fetchedAt = 0;
  private imported = new Map<string, Promise<CryptoKey>>();

  constructor(private readonly fetchJwks: JwksFetcher, private readonly clock: () => number = Date.now) {}

  private async refresh(): Promise<void> {
    let body: unknown;
    try {
      body = await this.fetchJwks();
    } catch {
      throw new IssuerTokenError("jwks_unavailable", "fetch");
    }
    this.keys = parseJwks(body);
    this.fetchedAt = this.clock();
    this.imported.clear();
  }

  private async key(kid: string): Promise<CryptoKey> {
    const stale = !this.keys || this.clock() - this.fetchedAt >= JWKS_MAX_AGE_MS;
    if (stale) await this.refresh();
    let jwk = this.keys?.get(kid);
    if (!jwk && !stale && this.clock() - this.fetchedAt >= UNKNOWN_KID_REFETCH_MS) {
      await this.refresh();
      jwk = this.keys?.get(kid);
    }
    if (!jwk) throw new IssuerTokenError("unknown_key", kid);
    let imported = this.imported.get(kid);
    if (!imported) {
      const { kty, crv, x, y } = jwk;
      imported = crypto.subtle.importKey("jwk", { kty, crv, x, y }, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
      this.imported.set(kid, imported);
    }
    return imported;
  }

  /**
   * Verify signature, `alg`, the exact `typ` and `exp` (when `requireExp`).
   * The caller checks every claim binding its use (account, hub, epoch…).
   */
  async verify(token: string, typ: IssuerTyp, options: { now?: number; requireExp?: boolean } = {}): Promise<VerifiedToken> {
    const parts = token.split(".");
    if (parts.length !== 3) throw new IssuerTokenError("malformed", "segments");
    const header = parseSegment(parts[0], "header");
    if (header.alg !== "ES256" || typeof header.kid !== "string") throw new IssuerTokenError("malformed", "header");
    if (header.typ !== typ) throw new IssuerTokenError("wrong_type", String(header.typ));
    if ("jwk" in header || "x5c" in header || "jku" in header) throw new IssuerTokenError("malformed", "embedded key");
    const claims = parseSegment(parts[1], "claims");
    const signature = b64urlDecode(parts[2], "signature");
    if (signature.length !== 64) throw new IssuerTokenError("bad_signature", "length");
    const key = await this.key(header.kid);
    const ok = await crypto.subtle.verify(
      { name: "ECDSA", hash: "SHA-256" },
      key,
      signature as BufferSource,
      encoder.encode(`${parts[0]}.${parts[1]}`),
    );
    if (!ok) throw new IssuerTokenError("bad_signature", typ);
    if (options.requireExp !== false) {
      const now = Math.floor((options.now ?? this.clock()) / 1000);
      if (typeof claims.exp !== "number" || claims.exp <= now) throw new IssuerTokenError("expired", typ);
    }
    return { header, claims };
  }
}
