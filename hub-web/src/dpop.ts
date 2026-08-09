import type { StoredMachine } from "./types";

const encoder = new TextEncoder();

function b64url(bytes: ArrayBuffer | Uint8Array): string {
  const input = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let binary = "";
  for (const value of input) binary += String.fromCharCode(value);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function encodedJson(value: unknown): string {
  return b64url(encoder.encode(JSON.stringify(value)));
}

export async function createDeviceKey(): Promise<{
  privateKey: CryptoKey;
  publicKey: JsonWebKey;
}> {
  const pair = await crypto.subtle.generateKey(
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign", "verify"],
  );
  const publicKey = await crypto.subtle.exportKey("jwk", pair.publicKey);
  return { privateKey: pair.privateKey, publicKey };
}

export async function dpopHeaders(
  machine: StoredMachine,
  method: string,
  targetUri: string,
): Promise<Record<string, string>> {
  const ath = b64url(await crypto.subtle.digest("SHA-256", encoder.encode(machine.credential)));
  const header = encodedJson({
    alg: "ES256",
    jwk: {
      kty: machine.publicKey.kty,
      crv: machine.publicKey.crv,
      x: machine.publicKey.x,
      y: machine.publicKey.y,
    },
  });
  const claims = encodedJson({
    htm: method.toUpperCase(),
    htu: targetUri,
    iat: Math.floor(Date.now() / 1000),
    jti: crypto.randomUUID(),
    ath,
  });
  const signature = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    machine.privateKey,
    encoder.encode(`${header}.${claims}`),
  );
  return {
    Authorization: `DPoP ${machine.credential}`,
    DPoP: `${header}.${claims}.${b64url(signature)}`,
  };
}
