import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { StoredMachine } from "./types";

// cas-d308: another tab rotates this browser's credential (generation 1 → 2)
// and commits it to the shared catalog. This tab still holds generation 1, so
// every request it already has in flight is refused as an unknown credential.
// Real Web Locks and IndexedDB are replaced by a serial lock and a catalog
// snapshot; signing, refusal reading and adoption remain the real code.
const shared = vi.hoisted(() => ({ installed: undefined as StoredMachine | undefined }));
vi.mock("./installation-access", async (importOriginal) => {
  const original = await importOriginal<typeof import("./installation-access")>();
  let tail: Promise<unknown> = Promise.resolve();
  return {
    ...original,
    installationLock: <T>(_hubId: string, run: () => Promise<T>) => {
      const next = tail.then(run, run);
      tail = next.catch(() => undefined);
      return next;
    },
    notifyInstallation: () => {},
  };
});
vi.mock("./storage", async (importOriginal) => {
  const original = await importOriginal<typeof import("./storage")>();
  return {
    ...original,
    catalog: { snapshot: async () => ({ machines: shared.installed ? [shared.installed] : [], pendingCleanup: 0 }) },
    installationStore: { list: async () => [] },
  };
});

const { HubConnectionSupervisor } = await import("./connection");
const { createDeviceKey } = await import("./dpop");

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  shared.installed = undefined;
});

async function machine(credentialId: string, credentialGeneration: number, key?: Awaited<ReturnType<typeof createDeviceKey>>): Promise<StoredMachine> {
  return {
    id: "atlas", label: "Atlas", baseUrl: "https://atlas.test",
    deviceId: "device-1", credentialId, credential: `opaque-${credentialId}`, credentialGeneration,
    expiresAt: new Date(Date.now() + 60_000).toISOString(), scopes: ["machine-read"],
    ...(key ?? await createDeviceKey()),
  };
}

describe("adopting a peer tab's rotated credential (cas-d308)", () => {
  it("retries every request refused with the stale credential, not only the one that adopted", async () => {
    vi.stubGlobal("crypto", webcrypto);
    vi.stubGlobal("window", globalThis);
    const key = await createDeviceKey();
    const stale = await machine("gen-1", 1, key);
    shared.installed = await machine("gen-2", 2, key);
    // Both requests leave with generation 1 before either is refused, as the
    // inventory read and a catalog poll did in the HUB-J2 M02 trace.
    let staleArrivals = 0;
    let releaseStale!: () => void;
    const bothStale = new Promise<void>((resolve) => { releaseStale = resolve; });
    const seen: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input)).pathname;
      const authorization = new Headers(init?.headers).get("Authorization");
      seen.push(`${path} ${authorization}`);
      if (authorization === "DPoP opaque-gen-1") {
        if (++staleArrivals === 2) releaseStale();
        await bothStale;
        return Response.json({ reason: "unknown_credential", retryable: false }, { status: 401 });
      }
      if (path === "/v1/sessions") return Response.json({ sessions: [] });
      if (path === "/v1/auth/devices") return Response.json([{ device_id: "device-1" }]);
      throw new Error(`unexpected request ${path}`);
    }));
    const connection = new HubConnectionSupervisor(stale, {
      onState: () => {}, onSessions: () => {}, onMachineEvent: () => {}, onSessionState: () => {},
      onOutput: () => {}, onPaneKeyframe: () => {}, onSocketError: () => {},
    });

    const [sessions, devices] = await Promise.all([
      connection.request<{ sessions: unknown[] }>("GET", "/v1/sessions"),
      connection.request<unknown[]>("GET", "/v1/auth/devices"),
    ]);

    expect(sessions).toEqual({ sessions: [] });
    expect(devices).toEqual([{ device_id: "device-1" }]);
    expect(stale.credentialId).toBe("gen-2");
    expect(seen.filter((line) => line.endsWith("opaque-gen-2")).sort()).toEqual(["/v1/auth/devices DPoP opaque-gen-2", "/v1/sessions DPoP opaque-gen-2"]);
    connection.stop();
  });

  it("still reads a refusal of the current credential as a lost pairing", async () => {
    vi.stubGlobal("crypto", webcrypto);
    vi.stubGlobal("window", globalThis);
    const current = await machine("gen-2", 2);
    shared.installed = { ...current };
    const used: (string | null)[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      used.push(new Headers(init?.headers).get("Authorization"));
      return Response.json({ reason: "revoked", retryable: false }, { status: 401 });
    }));
    const connection = new HubConnectionSupervisor(current, {
      onState: () => {}, onSessions: () => {}, onMachineEvent: () => {}, onSessionState: () => {},
      onOutput: () => {}, onPaneKeyframe: () => {}, onSocketError: () => {},
    });
    await expect(connection.request("GET", "/v1/auth/devices")).rejects.toMatchObject({ kind: "revoked", message: "pairing was revoked" });
    // No different credential exists to adopt, so nothing is retried with one.
    expect(new Set(used)).toEqual(new Set(["DPoP opaque-gen-2"]));
    connection.stop();
  });
});
