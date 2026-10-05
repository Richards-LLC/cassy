import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HubConnectionSupervisor, HubRequestError, ScopeGrantError, type HubCallbacks } from "./connection";
import { createDeviceKey } from "./dpop";
import type { StoredMachine } from "./types";

if (!globalThis.crypto?.subtle) vi.stubGlobal("crypto", webcrypto);
afterEach(() => vi.unstubAllGlobals());

async function machine(scopes: StoredMachine["scopes"]): Promise<StoredMachine> {
  const { privateKey, publicKey } = await createDeviceKey();
  return {
    id: "atlas", label: "Atlas · Linux", baseUrl: "https://atlas.test", deviceId: "device",
    credentialId: "credential-id", credential: "opaque", expiresAt: new Date(Date.now() + 60_000).toISOString(),
    scopes, publicKey, privateKey,
  };
}

describe("a refused hub request says what the hub said (cas-d382, fleet-operations brief)", () => {
  it("surfaces the {error, detail} body instead of only the status", async () => {
    vi.stubGlobal("fetch", async () => new Response(JSON.stringify({ error: "stale", detail: "swift-lark-3 already restarted" }), { status: 409 }));
    const connection = new HubConnectionSupervisor(await machine(["session-read"]), {} as HubCallbacks);
    const refused = await connection.status("patient-pelican-9").catch((error: unknown) => error);
    expect(refused).toBeInstanceOf(HubRequestError);
    expect(refused).toMatchObject({ status: 409, code: "stale", detail: "swift-lark-3 already restarted" });
    // The message keeps its old wording for anything that logs it.
    expect((refused as Error).message).toBe("GET /v1/sessions/patient-pelican-9/status failed (409)");
  });

  it("reads `reason` as the detail, and leaves both unset for a body that is not JSON", async () => {
    let body = JSON.stringify({ error: "audit_unavailable", reason: "audit log is read-only" });
    vi.stubGlobal("fetch", async () => new Response(body, { status: 503 }));
    const connection = new HubConnectionSupervisor(await machine(["session-read"]), {} as HubCallbacks);
    expect(await connection.status("s").catch((error: unknown) => error)).toMatchObject({ status: 503, code: "audit_unavailable", detail: "audit log is read-only" });
    body = "<html>bad gateway</html>";
    const plain = await connection.status("s").catch((error: unknown) => error) as HubRequestError;
    expect(plain).toBeInstanceOf(HubRequestError);
    expect(plain.status).toBe(503);
    expect(plain.code).toBeUndefined();
    expect(plain.detail).toBeUndefined();
  });

  it("allows managing workers once, as session launch is allowed, and keeps the new scopes", async () => {
    const control = await machine(["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"]);
    const sent: unknown[] = [];
    vi.stubGlobal("fetch", async (_input: URL | string, init?: RequestInit) => {
      sent.push(JSON.parse(String(init?.body)));
      return new Response(JSON.stringify({ scopes: [...control.scopes, "factory-operate"] }), { status: 200 });
    });
    const refreshed = vi.fn();
    const connection = new HubConnectionSupervisor(control, { onCredentialRefreshed: refreshed } as unknown as HubCallbacks);
    await connection.enableFactoryOperate();
    expect(sent).toEqual([{ add: ["factory-operate"] }]);
    expect(control.scopes).toContain("factory-operate");
    expect(refreshed).toHaveBeenCalledWith(control);
  });

  it("says a refused grant needs a control invitation, keeping the hub's code", async () => {
    vi.stubGlobal("fetch", async () => new Response(JSON.stringify({ error: "scope_denied", detail: "control scopes required" }), { status: 403 }));
    const connection = new HubConnectionSupervisor(await machine(["machine-read", "session-read", "pane-read"]), {} as HubCallbacks);
    const refused = await connection.enableFactoryOperate().catch((error: unknown) => error);
    expect(refused).toBeInstanceOf(ScopeGrantError);
    expect(refused).toMatchObject({ status: 403, code: "scope_denied", detail: "control scopes required", message: "This pairing can't allow managing workers. Pair with a control invitation, then try again." });
  });
});
