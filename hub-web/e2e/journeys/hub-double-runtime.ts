// Node adapter for protocol fixtures. The clickable preview selects its browser
// adapter in vite.preview.config.ts; HubDouble itself has only type-only runner imports.
import { createHash, webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";
import { journeyNow } from "./clock";

export const runtime = {
  now: journeyNow,
  visualPageCsp: readFileSync(new URL("../../../cas-cli/src/hub/visual_page_csp.txt", import.meta.url), "utf8").trim(),
  hash: (value: string) => createHash("sha256").update(value).digest("base64url"),
  async verifyInstallation(key: JsonWebKey, proof: string, transcript: unknown[]): Promise<boolean> {
    try {
      const imported = await webcrypto.subtle.importKey("jwk", key, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
      return await webcrypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, imported, Buffer.from(proof, "base64url"), new TextEncoder().encode(JSON.stringify(transcript)));
    } catch { return false; }
  },
};
