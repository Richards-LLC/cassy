import { createHash, webcrypto } from "node:crypto";
import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";
import { createDeviceKey, dpopHeaders } from "./dpop";
import { consumePairingFragment } from "./fragment";
import type { StoredMachine } from "./types";

Object.defineProperty(globalThis, "crypto", { value: webcrypto, configurable: true });

describe("binding Commander browser invariants", () => {
  it("H4-CATALOG-01 consumes pairing fragments synchronously and preserves no capability in the URL", () => {
    let replacement = "";
    const location = { hash: "#pair=one-time-secret&hub=machine-1", pathname: "/", search: "" } as Location;
    const history = { replaceState: (_: unknown, __: string, path: string) => { replacement = path; } } as unknown as History;
    expect(consumePairingFragment(location, history)).toEqual({ token: "one-time-secret", hubId: "machine-1" });
    expect(replacement).toBe("/");
    expect(replacement).not.toContain("one-time-secret");
  });

  it("H4-STORAGE-02 creates a non-extractable P-256 signing key and valid proof", async () => {
    const { privateKey, publicKey } = await createDeviceKey();
    expect(privateKey.extractable).toBe(false);
    await expect(crypto.subtle.exportKey("jwk", privateKey)).rejects.toThrow();
    const machine = {
      id: "machine", label: "Machine", baseUrl: "https://hub.example", deviceId: "device",
      credentialId: "credential-id", credential: "opaque-credential", expiresAt: new Date(Date.now() + 60_000).toISOString(),
      scopes: ["machine-read"], publicKey, privateKey,
    } satisfies StoredMachine;
    const headers = await dpopHeaders(machine, "GET", "/v1/machine");
    const [encodedHeader, encodedClaims, encodedSignature] = headers.DPoP.split(".");
    const decode = (value: string) => Buffer.from(value, "base64url");
    expect(JSON.parse(decode(encodedClaims).toString())).toMatchObject({ htm: "GET", htu: "/v1/machine" });
    const imported = await crypto.subtle.importKey("jwk", publicKey, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, imported, decode(encodedSignature), new TextEncoder().encode(`${encodedHeader}.${encodedClaims}`))).toBe(true);
  });

  it("pins the green Ghostty WASM spike artifacts by integrity", async () => {
    const cases = [
      ["terminal/ghostty/vendor/ghostty-vt.wasm", "6b1df1a96d59adc26360c312924898dbc122f980c17a32eb1624e48795b83f7e"],
      ["terminal/ghostty/vendor/ghostty-write-pty.wasm", "75cb147e98ede3f85f3cd6236a30f6d12565b0b237e1d8db941f5f3e8ad3d903"],
    ];
    for (const [path, expected] of cases) {
      const bytes = await readFile(new URL(path, import.meta.url));
      expect(createHash("sha256").update(bytes).digest("hex")).toBe(expected);
    }
  });

  it("keeps long-lived credentials out of ambient browser storage and URL channels", async () => {
    const source = await Promise.all(["storage.ts", "main.ts", "dpop.ts"].map((path) => readFile(new URL(path, import.meta.url), "utf8")));
    const joined = source.join("\n");
    for (const forbidden of ["local" + "Storage", "session" + "Storage", "document.cookie", "serviceWorker.register", "caches.open"]) {
      expect(joined).not.toContain(forbidden);
    }
    expect(joined).toContain("indexedDB.open");
  });

  it("targets interrupt at the explicitly selected pane rather than render order", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("selectedPanes.get(sessionKey(selected.id, selectedSession))");
    expect(source).toContain("{ InterruptPane: { pane_id: pane } }");
    expect(source).not.toContain("[...surfaces.keys()].find");
  });

  it("never caches an asynchronously-created terminal against a detached render", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("existingSurface.element !== mount || !existingSurface.element.isConnected");
    expect(source).toContain("!mount.isConnected || currentMount !== mount");
    expect(source).toContain("surface.dispose();");
  });

  it("preserves the active pane grid across lease and status renders", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("currentGrid?.dataset.sessionKey === terminalSessionKey");
    expect(source).toContain("replaceWith(preservedGrid)");
    expect(source).toContain("data-session-key");
  });

  it("resends the live grid size when observer mode becomes controller mode", async () => {
    const source = await readFile(new URL("main.ts", import.meta.url), "utf8");
    expect(source).toContain("const becameController = state.held_by_me && !leases.get(key)?.held_by_me");
    expect(source).toContain("if (becameController) resizeControlledPanes(machineId, session)");
    expect(source).toContain("{ ResizePane: { pane_id: pane.id, cols: surface.cols, rows: surface.rows } }");
  });
});
