// Operator relay proof of possession (cloud contract §4.2, cas-9b7d S1).
//
// Every device relay call carries `Authorization: PSC-PoP <grant_id>` and a
// compact ES256 JWS in `PSC-PoP-Proof`, signed by the device's own relay key.
// It is not the hub's DPoP (dpop.ts): another `typ`, header, key and audience,
// and the key is looked up by thumbprint (`kid`), never sent as `jwk`.

import { b64urlEncode } from "./hpke";

const encoder = new TextEncoder();

export const POP_TYP = "psc-op-pop+jwt";
/** SHA-256 of zero bytes, the `bdg` of an empty body (§4.2). */
export const EMPTY_BODY_DIGEST = "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU";
/** Proof lifetime; the server accepts at most 120 s (§4.2). */
const PROOF_LIFETIME_S = 60;

export interface SigningKey {
  /** Non-extractable ECDSA P-256 private key; structured-clonable into IndexedDB. */
  privateKey: CryptoKey;
  /** Public JWK (kty, crv, x, y only). */
  publicJwk: { kty: "EC"; crv: "P-256"; x: string; y: string };
  /** RFC 7638 SHA-256 thumbprint, base64url: the grant's `signing_jkt`. */
  jkt: string;
}

/** RFC 7638 thumbprint of a P-256 public JWK (members in lexicographic order). */
export async function jwkThumbprint(jwk: { crv: string; kty: string; x: string; y: string }): Promise<string> {
  const canonical = `{"crv":"${jwk.crv}","kty":"${jwk.kty}","x":"${jwk.x}","y":"${jwk.y}"}`;
  return b64urlEncode(await crypto.subtle.digest("SHA-256", encoder.encode(canonical)));
}

export async function createSigningKey(): Promise<SigningKey> {
  const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
  const exported = await crypto.subtle.exportKey("jwk", pair.publicKey);
  if (exported.kty !== "EC" || exported.crv !== "P-256" || !exported.x || !exported.y) {
    throw new Error("WebCrypto returned an unexpected P-256 public key");
  }
  const publicJwk = { kty: "EC" as const, crv: "P-256" as const, x: exported.x, y: exported.y };
  return { privateKey: pair.privateKey, publicJwk, jkt: await jwkThumbprint(publicJwk) };
}

export async function bodyDigest(body: Uint8Array): Promise<string> {
  return b64urlEncode(await crypto.subtle.digest("SHA-256", body as BufferSource));
}

export type ProofBinding =
  | { kind: "grant"; grantId: string; generation: string }
  | { kind: "enrollment"; enrollmentId: string };

export interface ProofRequest {
  /** `<PSC base URL>/api/operator`, exact. */
  audience: string;
  method: string;
  /** Path and query exactly as sent, percent-encoding preserved. */
  pathAndQuery: string;
  /** The raw request body bytes (zero bytes when there is no body). */
  body: Uint8Array;
  binding: ProofBinding;
  /** Signing time in ms, already corrected for observed server skew. */
  now: number;
}

function encodedJson(value: unknown): string {
  return b64urlEncode(encoder.encode(JSON.stringify(value)));
}

export async function popProof(key: SigningKey, request: ProofRequest): Promise<string> {
  const iat = Math.floor(request.now / 1000);
  const header = encodedJson({ typ: POP_TYP, alg: "ES256", kid: key.jkt });
  const claims: Record<string, string | number> = {
    aud: request.audience,
    htm: request.method.toUpperCase(),
    htp: request.pathAndQuery,
    bdg: request.body.length === 0 ? EMPTY_BODY_DIGEST : await bodyDigest(request.body),
    iat,
    exp: iat + PROOF_LIFETIME_S,
    jti: b64urlEncode(crypto.getRandomValues(new Uint8Array(16))),
  };
  if (request.binding.kind === "grant") {
    claims.gid = request.binding.grantId;
    claims.gen = request.binding.generation;
  } else {
    claims.enr = request.binding.enrollmentId;
  }
  const signingInput = `${header}.${encodedJson(claims)}`;
  // WebCrypto ECDSA emits IEEE P1363 r‖s, which is the JWS ES256 encoding.
  const signature = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key.privateKey, encoder.encode(signingInput));
  return `${signingInput}.${b64urlEncode(signature)}`;
}
