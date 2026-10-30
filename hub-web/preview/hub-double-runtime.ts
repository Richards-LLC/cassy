import visualPageCsp from "../../cas-cli/src/hub/visual_page_csp.txt?raw";
import { createHash } from "./crypto-shim";

// Same protocol, browser primitives. No Node globals or test runner at runtime.
export const runtime: typeof import("../e2e/journeys/hub-double-runtime").runtime = {
  now: () => Date.now(),
  visualPageCsp: visualPageCsp.trim(),
  hash: (value) => createHash("sha256").update(value).digest("base64url"),
  async verifyInstallation(key, proof, transcript) {
    try {
      const imported = await crypto.subtle.importKey("jwk", key, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
      const base64 = proof.replace(/-/g, "+").replace(/_/g, "/");
      const signature = Uint8Array.from(atob(base64), c => c.charCodeAt(0));
      return await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, imported, signature, new TextEncoder().encode(JSON.stringify(transcript)));
    } catch { return false; }
  },
};
