import { webcrypto } from "node:crypto";
import { beforeAll, describe, expect, it } from "vitest";
import { InstallationAccess, type InstallationRecord, type InstallationStore, type InstallationLock, installationTranscript } from "./installation-access";
import { MachineCatalog, type MachineCatalogRecord } from "./storage";
import { PairingCleanupError, type ExchangeOptions } from "./pairing-exchange";
import type { StoredMachine } from "./types";

beforeAll(() => Object.defineProperty(globalThis, "crypto", { value: webcrypto, configurable: true }));
function fixture() {
  const rows = new Map<string, MachineCatalogRecord>();
  const keys = new Map<string, InstallationRecord>();
  const catalog = new MachineCatalog({ list: async () => [...rows.values()], update: async (id, update) => { const value = update(rows.get(id)); if (value) rows.set(id, structuredClone(value)); else rows.delete(id); } });
  const store: InstallationStore = { get: async (id) => keys.get(id) && structuredClone(keys.get(id)!), put: async (r) => { keys.set(r.id, structuredClone(r)); }, list: async () => [...keys.values()].map((r) => structuredClone(r)) };
  let tail: Promise<unknown> = Promise.resolve();
  const lock: InstallationLock = async (_id, run) => { const predecessor = tail; let release!: () => void; tail = new Promise<void>((r) => { release = r; }); await predecessor; try { return await run(); } finally { release(); } };
  const access = new InstallationAccess(store, catalog, lock);
  return { catalog, rows, keys, store, access, lock };
}
const credential = (generation: number, secret: string) => ({ device_id: "stable-device", credential_id: `credential-${generation}`, credential_generation: generation, credential: secret, expires_at: "2030-01-01T00:00:00Z", scopes: ["machine-read"], account_enrollment: { state: "unenrolled" } });
function options(f: ReturnType<typeof fixture>, fetcher: ExchangeOptions["fetcher"], overrides: Partial<ExchangeOptions> = {}): ExchangeOptions {
  return { invitation: { kind: "invitation", token: "A".repeat(43), hubId: "hub", hubUrl: "https://soundwave.example", controllerOrigin: "https://commander.example", scopes: ["machine-read"] }, controllerOrigin: "https://commander.example", deviceLabel: "Phone", operatorLabel: "Operator", fetcher: (input, init) => String(input).endsWith("/v1/health") ? Promise.resolve(json({ installation_protocol: 1 })) : fetcher(input, init),
    createKey: async () => { throw new Error("must use retained installation key"); }, installationGeneration: 1,
    stagePersisted: (m, i) => f.catalog.stage(m, i), activatePersisted: (i, s) => f.catalog.activate(i, s), rollbackPersisted: (i) => f.catalog.rollback(i), ...overrides };
}
const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });

describe("installation transaction using real WebCrypto and catalog code", () => {
  it("five repairs retain a nonextractable key, exact device and increasing credential generation", async () => {
    const f = fixture();
    let generation = 0;
    let firstJwk: JsonWebKey | undefined;
    const fetcher: ExchangeOptions["fetcher"] = async (input, init) => {
      const path = new URL(String(input)).pathname;
      const request = JSON.parse(String(init?.body));
      if (path.endsWith("exchange")) {
        generation++;
        firstJwk ??= request.public_key_jwk;
        expect(request.public_key_jwk).toEqual(firstJwk);
        expect(request.installation.expected_generation).toBe(generation - 1);
        expect(request.installation.device_id).toBe(generation === 1 ? null : "stable-device");
        const record = [...f.keys.values()][0]!;
        const prior = (await f.catalog.snapshot()).machines[0];
        const transcript = await installationTranscript(options(f, fetcher), record, prior, request.installation.operation_id, request.installation.credential, ["machine-read"]);
        const key = await crypto.subtle.importKey("jwk", request.public_key_jwk, { name: "ECDSA", namedCurve: "P-256" }, true, ["verify"]);
        expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, key, Buffer.from(request.installation.proof, "base64url"), new TextEncoder().encode(JSON.stringify(transcript)))).toBe(true);
        return json(credential(generation, request.installation.credential));
      }
      expect(path.endsWith("commit")).toBe(true);
      return new Response(null, { status: 204 });
    };
    for (let n = 1; n <= 5; n++) {
      const installed = await f.access.pair(options(f, fetcher));
      expect(installed.credentialGeneration).toBe(n);
      expect(installed.privateKey.extractable).toBe(false);
      expect((await f.catalog.snapshot()).machines).toHaveLength(1);
    }
    expect(f.keys.size).toBe(1);
  });

  it("cancellation after server commit aborts remotely before restoring the old catalog", async () => {
    const f = fixture();
    let generation = 0;
    const calls: string[] = [];
    let canceled = false;
    const fetcher: ExchangeOptions["fetcher"] = async (input, init) => {
      const path = new URL(String(input)).pathname; calls.push(path);
      if (path.endsWith("exchange")) { generation++; return json(credential(generation, JSON.parse(String(init?.body)).installation.credential)); }
      if (path.endsWith("commit") && generation === 2) canceled = true;
      if (path.endsWith("abort")) expect((await f.catalog.snapshot()).machines[0]?.credentialGeneration).toBe(1);
      return new Response(null, { status: 204 });
    };
    const old = await f.access.pair(options(f, fetcher));
    await expect(f.access.pair(options(f, fetcher, { isCurrent: () => !canceled }))).rejects.toThrow("cancelled");
    expect((await f.catalog.snapshot()).machines[0]).toEqual(old);
    expect(calls.filter((path) => path.endsWith("abort")).length).toBeGreaterThan(0);
    expect([...f.keys.values()][0]!.pending).toBeUndefined();
  });

  it("a lost prepare response leaves a recoverable operation when remote abort is unavailable", async () => {
    const f = fixture();
    const broken: ExchangeOptions["fetcher"] = async () => { throw new TypeError("network unavailable"); };
    await expect(f.access.pair(options(f, broken))).rejects.toBeInstanceOf(PairingCleanupError);
    expect([...f.keys.values()][0]!.pending).toBeDefined();
    const recovered: string[] = [];
    expect(await f.access.recover(async (input) => { recovered.push(String(input)); return new Response(null, { status: 204 }); })).toBe(0);
    expect(recovered[0]).toContain("/abort");
    expect([...f.keys.values()][0]!.pending).toBeUndefined();
  });

  it("two tabs serialize, reread the winner and never stage from a stale generation", async () => {
    const f = fixture();
    const peer = new InstallationAccess(f.store, f.catalog, f.lock);
    let generation = 0;
    const expected: number[] = [];
    const fetcher: ExchangeOptions["fetcher"] = async (input, init) => {
      if (String(input).endsWith("exchange")) {
        const proof = JSON.parse(String(init?.body)).installation;
        expected.push(proof.expected_generation); generation++;
        return json(credential(generation, proof.credential));
      }
      return new Response(null, { status: 204 });
    };
    await Promise.all([f.access.pair(options(f, fetcher)), peer.pair(options(f, fetcher))]);
    expect(expected).toEqual([0, 1]);
    expect((await f.catalog.snapshot()).machines[0]?.credentialGeneration).toBe(2);
  });

  it("key rotation proves both keys and cancellation restores the prior durable signing key", async () => {
    const f = fixture(); let generation = 0; let cancel = false;
    const fetcher: ExchangeOptions["fetcher"] = async (input, init) => {
      const request = JSON.parse(String(init?.body));
      if (String(input).endsWith("exchange")) {
        generation++;
        if (generation === 2) {
          const record = [...f.keys.values()][0]!;
          const prior = (await f.catalog.snapshot()).machines[0]!;
          const transcript = await installationTranscript(options(f, fetcher), record, prior, request.installation.operation_id, request.installation.credential, ["machine-read"]);
          const oldKey = await crypto.subtle.importKey("jwk", prior.publicKey, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
          expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, oldKey, Buffer.from(request.installation.previous_proof, "base64url"), new TextEncoder().encode(JSON.stringify(transcript)))).toBe(true);
          expect(record.publicKey.x).not.toBe(prior.publicKey.x);
        }
        return json(credential(generation, request.installation.credential));
      }
      if (String(input).endsWith("commit") && generation === 2) cancel = true;
      return new Response(null, { status: 204 });
    };
    const old = await f.access.pair(options(f, fetcher));
    await expect(f.access.pair(options(f, fetcher, { rotateKey: true, isCurrent: () => !cancel }))).rejects.toThrow("cancelled");
    expect([...f.keys.values()][0]!.publicKey).toEqual(old.publicKey);
    expect((await f.catalog.snapshot()).machines[0]).toEqual(old);
  });

  it("local catalog removal retains installation identity for explicit re-pair", async () => {
    const f = fixture(); let generation = 0;
    const fetcher: ExchangeOptions["fetcher"] = async (input, init) => {
      if (String(input).endsWith("exchange")) {
        const proof = JSON.parse(String(init?.body)).installation; generation++;
        if (generation === 2) { expect(proof.device_id).toBe("stable-device"); expect(proof.expected_generation).toBe(1); }
        return json(credential(generation, proof.credential));
      }
      return new Response(null, { status: 204 });
    };
    await f.access.pair(options(f, fetcher)); await f.catalog.remove("hub");
    const repaired = await f.access.pair(options(f, fetcher));
    expect(repaired.deviceId).toBe("stable-device");
  });

  it("catalog rejects late refresh overwrites from old generation or another installation", async () => {
    const f = fixture();
    const machine = { id: "hub", deviceId: "stable-device", credentialId: "new", credentialGeneration: 4 } as StoredMachine;
    await f.catalog.put(machine);
    await f.catalog.put({ ...machine, credentialGeneration: 3, credentialId: "stale" });
    await f.catalog.put({ ...machine, deviceId: "other", credentialGeneration: 8, credentialId: "other" });
    expect((await f.catalog.snapshot()).machines[0]).toEqual(machine);
  });
});
