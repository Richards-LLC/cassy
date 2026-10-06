// Bind a paired hub installation to this browser's operator-inbox account
// device (cloud contract §5.5, cas-4634).
//
// 1. The hub issues a one-use challenge to this installation's DPoP session.
// 2. The cloud signs a `psc-op-enrollment+jwt` naming that hub, the
//    challenge, and the thumbprint of this browser's hub installation key.
// 3. The hub verifies it (issuer JWKS, audience, challenge, installation key,
//    origin, and its own machine account) and records the installation as
//    enrolled. Only the thumbprint crosses to the cloud; the hub credential
//    and DPoP proofs never do.

import type { AccountEnrollment } from "../installation-access";
import { jwkThumbprint } from "./pop";
import { OperatorWireError, str, type OperatorClient } from "./wire";

export interface HubAccountClient {
  request<T>(method: string, path: string, body?: unknown): Promise<T>;
}

export type HubEnrollmentOutcome =
  | { kind: "enrolled"; enrollment: AccountEnrollment }
  /** The hub has no machine in this account's inbox (not an error). */
  | { kind: "hub_not_in_account" }
  | { kind: "refused"; reason: string };

export async function enrollPairedInstallation(
  hub: HubAccountClient,
  cloud: OperatorClient,
  installationKey: JsonWebKey,
): Promise<HubEnrollmentOutcome> {
  if (installationKey.kty !== "EC" || installationKey.crv !== "P-256" || !installationKey.x || !installationKey.y) {
    return { kind: "refused", reason: "installation_key_unsupported" };
  }
  const challenge = await hub.request<{ hub_id: string; hub_challenge: string }>("POST", "/v1/auth/account/challenge", {});
  const installationJkt = await jwkThumbprint({ kty: "EC", crv: "P-256", x: installationKey.x, y: installationKey.y });
  let assertion: string;
  try {
    const response = await cloud.request({
      method: "POST",
      path: "/api/operator/assertions",
      body: { wire_version: 1, hub_id: challenge.hub_id, hub_challenge: challenge.hub_challenge, installation_jkt: installationJkt },
      auth: "grant",
    });
    assertion = str(response.assertion, "assertion");
  } catch (error) {
    if (error instanceof OperatorWireError && error.code === "hub_not_enrolled") return { kind: "hub_not_in_account" };
    throw error;
  }
  try {
    const bound = await hub.request<{ account_enrollment: AccountEnrollment }>("POST", "/v1/auth/account/enrollment", { assertion });
    return { kind: "enrolled", enrollment: bound.account_enrollment };
  } catch (error) {
    const code = (error as { code?: unknown }).code;
    return { kind: "refused", reason: typeof code === "string" ? code : "hub_refused" };
  }
}
