import { describe, expect, it } from "vitest";
import { OperatorCloudDouble } from "../../test/operator-cloud-double";
import { completeEnrollment, pollEnrollment, startEnrollment } from "./enrollment";
import { enrollPairedInstallation } from "./hub-enrollment";
import { exportPublicKey, generateKeyPair } from "./hpke";
import { ISSUER_TYP, IssuerKeys } from "./issuer";
import { jwkThumbprint } from "./pop";
import { MemoryInboxStore } from "./store";
import { OperatorClient } from "./wire";

const PAGE_ORIGIN = "https://hub.petrastella.io";

async function signedIn(cloud: OperatorCloudDouble) {
  const client = new OperatorClient({ origin: cloud.baseUrl, fetch: cloud.fetchFor(PAGE_ORIGIN) });
  const store = new MemoryInboxStore();
  const pending = await startEnrollment(client, store, { pageOrigin: PAGE_ORIGIN, label: "phone" });
  await cloud.approve(pending.userCode);
  await pollEnrollment(client, pending);
  await completeEnrollment(client, store, new IssuerKeys(() => cloud.jwks()), pending, null);
  return client;
}

async function installationKey(): Promise<JsonWebKey> {
  const pair = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"])) as CryptoKeyPair;
  return crypto.subtle.exportKey("jwk", pair.publicKey);
}

describe("hub installation enrollment (cas-4634, contract §5.5)", () => {
  it("relays the hub's one-use challenge and this installation's thumbprint, then hands the hub the cloud assertion", async () => {
    const cloud = new OperatorCloudDouble();
    await cloud.enrollMachine("hub-1", ["p1"], await exportPublicKey((await generateKeyPair()).publicKey));
    const client = await signedIn(cloud);
    const key = await installationKey();
    const calls: [string, string, unknown][] = [];
    const hub = {
      async request<T>(method: string, path: string, body?: unknown): Promise<T> {
        calls.push([method, path, body]);
        if (path === "/v1/auth/account/challenge") return { hub_id: "hub-1", hub_challenge: "challenge-1" } as T;
        return { account_enrollment: { state: "enrolled", account_id: cloud.accountId, relay_device_id: "d", grant_generation: "1", feed_generation: "1", epoch: "1", verified_at: "now" } } as T;
      },
    };
    const outcome = await enrollPairedInstallation(hub, client, key);
    expect(outcome.kind).toBe("enrolled");
    const [, , submitted] = calls[1];
    const { claims } = await new IssuerKeys(() => cloud.jwks()).verify((submitted as { assertion: string }).assertion, ISSUER_TYP.enrollment);
    expect(claims).toMatchObject({ aud: "cas-hub:hub-1", hub: "hub-1", chl: "challenge-1", origin: PAGE_ORIGIN, acct: cloud.accountId });
    expect(claims.ins_jkt).toBe(await jwkThumbprint({ kty: "EC", crv: "P-256", x: key.x!, y: key.y! }));
  });

  it("leaves a hub outside the account alone", async () => {
    const cloud = new OperatorCloudDouble();
    const client = await signedIn(cloud);
    const hub = {
      async request<T>(_method: string, path: string): Promise<T> {
        if (path === "/v1/auth/account/challenge") return { hub_id: "hub-elsewhere", hub_challenge: "c" } as T;
        throw new Error("the hub must not receive anything else");
      },
    };
    expect(await enrollPairedInstallation(hub, client, await installationKey())).toEqual({ kind: "hub_not_in_account" });
  });

  it("reports the hub's refusal code without retrying", async () => {
    const cloud = new OperatorCloudDouble();
    await cloud.enrollMachine("hub-1", ["p1"], await exportPublicKey((await generateKeyPair()).publicKey));
    const client = await signedIn(cloud);
    const hub = {
      async request<T>(_method: string, path: string): Promise<T> {
        if (path === "/v1/auth/account/challenge") return { hub_id: "hub-1", hub_challenge: "c" } as T;
        throw Object.assign(new Error("refused"), { code: "account_mismatch" });
      },
    };
    expect(await enrollPairedInstallation(hub, client, await installationKey())).toEqual({ kind: "refused", reason: "account_mismatch" });
  });
});
