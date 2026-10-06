// Device enrollment into the operator inbox (cloud contract §5.1–§5.3, §6.4).
//
// 1. `startEnrollment` creates the two keys (relay signing, HPKE encryption),
//    posts the unauthenticated challenge bound to this exact page origin, and
//    persists the pending state, so a reload keeps the same code.
// 2. The operator approves the code on the PSC approval page (or with
//    `cas operator approve <code>`); account permission is the only authority.
// 3. `pollEnrollment` follows the server interval (slow_down widens it).
// 4. `completeEnrollment` proves both keys (PoP with `enr`, the opened
//    encryption-key check), stores the grant, then fetches every retained
//    epoch key (`refreshEpochKeys`). No older device or hub takes part.

import { InboxCryptoError, b64urlDecode, b64urlEncode, exportPublicKey, generateKeyPair, openEnrollmentCheck, openEpochWrap } from "./hpke";
import { ISSUER_TYP, type IssuerKeys } from "./issuer";
import { createSigningKey } from "./pop";
import type { EpochKey, InboxIdentity, InboxStore, PendingEnrollment } from "./store";
import { OperatorWireError, decimal, list, parseGrant, record, str, type OperatorClient } from "./wire";

export type PollOutcome =
  | { status: "pending"; intervalS: number }
  | { status: "approved"; emailHint: string | null }
  | { status: "denied" }
  | { status: "expired" };

export async function startEnrollment(
  client: OperatorClient,
  store: InboxStore,
  input: { pageOrigin: string; label: string; email?: string },
): Promise<PendingEnrollment> {
  const signingKey = await createSigningKey();
  const encryption = await generateKeyPair();
  const response = await client.createEnrollment({
    origin: input.pageOrigin,
    device_label: input.label.trim().slice(0, 80),
    signing_jwk: signingKey.publicJwk,
    encryption_public_key: b64urlEncode(await exportPublicKey(encryption.publicKey)),
    ...(input.email ? { email: input.email } : {}),
  });
  const check = record(response.encryption_key_check, "encryption_key_check");
  const pending: PendingEnrollment = {
    cloudOrigin: client.origin,
    enrollmentId: str(response.enrollment_id, "enrollment_id"),
    userCode: str(response.user_code, "user_code"),
    pollSecret: str(response.poll_secret, "poll_secret"),
    approvalUrl: str(response.approval_url, "approval_url"),
    expiresAt: str(response.expires_at, "expires_at"),
    intervalS: typeof response.interval === "number" && response.interval > 0 ? response.interval : 5,
    check: { enc: str(check.enc, "enc"), ct: str(check.ct, "ct") },
    signingKey,
    encryption,
    label: input.label,
    createdAt: new Date().toISOString(),
  };
  if (!/^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/.test(pending.userCode)) {
    throw new OperatorWireError(0, "invalid_response", { field: "user_code" }, null);
  }
  const approval = new URL(pending.approvalUrl);
  if (approval.origin !== client.origin) throw new OperatorWireError(0, "invalid_response", { field: "approval_url" }, null);
  await store.savePending(pending);
  return pending;
}

export async function pollEnrollment(client: OperatorClient, pending: PendingEnrollment): Promise<PollOutcome> {
  try {
    const response = await client.pollEnrollment(pending.enrollmentId, pending.pollSecret);
    const status = str(response.status, "status");
    if (status === "approved") {
      return { status: "approved", emailHint: typeof response.account_email_hint === "string" ? response.account_email_hint : null };
    }
    if (status === "denied") return { status: "denied" };
    const interval = typeof response.interval === "number" && response.interval > 0 ? response.interval : pending.intervalS;
    return { status: "pending", intervalS: interval };
  } catch (error) {
    if (error instanceof OperatorWireError) {
      if (error.code === "slow_down") return { status: "pending", intervalS: pending.intervalS + 5 };
      if (error.code === "enrollment_expired" || error.code === "enrollment_not_found") return { status: "expired" };
    }
    throw error;
  }
}

export async function completeEnrollment(
  client: OperatorClient,
  store: InboxStore,
  issuer: IssuerKeys,
  pending: PendingEnrollment,
  emailHint: string | null,
): Promise<InboxIdentity> {
  const check = await openEnrollmentCheck(pending.encryption, pending.check, pending.enrollmentId);
  const response = await client.completeEnrollment(pending.enrollmentId, pending.signingKey, pending.pollSecret, check);
  const grant = parseGrant(response.grant);
  const deviceId = str(response.device_id, "device_id");
  const accountId = str(response.account_id, "account_id");
  if (grant.grantId !== deviceId || grant.accountId !== accountId || grant.kind !== "device") {
    throw new OperatorWireError(0, "invalid_response", { field: "grant" }, null);
  }
  const identity: InboxIdentity = {
    cloudOrigin: pending.cloudOrigin,
    accountId,
    deviceId,
    grant,
    signingKey: pending.signingKey,
    encryption: pending.encryption,
    feedGeneration: decimal(response.feed_generation, "feed_generation"),
    enrolledAt: new Date().toISOString(),
    label: pending.label,
    emailHint,
  };
  await store.saveIdentity(identity);
  await store.savePending(null);
  client.credential = { key: identity.signingKey, grantId: grant.grantId, generation: grant.generation };
  await refreshEpochKeys(client, store, issuer, identity);
  return identity;
}

export class EpochKeyError extends Error {
  constructor(readonly reason: "manifest_invalid" | "policy_regression" | "generation_changed" | "unwrap_failed", detail: string) {
    super(`${reason}: ${detail}`);
    this.name = "EpochKeyError";
  }
}

/**
 * Fetch and unwrap every retained epoch key of the current feed generation
 * (§6.4). Each wrap is accepted only with a verified manifest naming this
 * account, generation and epoch; the policy version may never go down. An
 * epoch listed in `unavailable_epochs` is reported, not invented.
 */
export async function refreshEpochKeys(
  client: OperatorClient,
  store: InboxStore,
  issuer: IssuerKeys,
  identity: InboxIdentity,
): Promise<{ epochs: string[]; unavailable: string[]; activeEpoch: string; feedGeneration: string }> {
  const response = await client.keyWraps();
  const feedGeneration = decimal(response.feed_generation, "feed_generation");
  const policyVersion = decimal(response.policy_version, "policy_version");
  const activeEpoch = decimal(response.active_epoch, "active_epoch");
  if (feedGeneration !== identity.feedGeneration) {
    throw new EpochKeyError("generation_changed", `${identity.feedGeneration} -> ${feedGeneration}`);
  }
  const keys: EpochKey[] = [];
  for (const entry of list(response.epochs, "epochs")) {
    const wrap = record(entry, "epoch");
    const epoch = decimal(wrap.epoch, "epoch");
    let claims: Record<string, unknown>;
    try {
      ({ claims } = await issuer.verify(str(wrap.manifest, "manifest"), ISSUER_TYP.epochManifest));
    } catch (error) {
      throw new EpochKeyError("manifest_invalid", `epoch ${epoch}: ${String(error)}`);
    }
    if (claims.acct !== identity.accountId || claims.fgen !== feedGeneration || claims.epoch !== epoch || typeof claims.pk !== "string") {
      throw new EpochKeyError("manifest_invalid", `epoch ${epoch}: binding`);
    }
    const suite = claims.suite as { kem?: unknown; kdf?: unknown; aead?: unknown } | undefined;
    if (suite?.kem !== 16 || suite.kdf !== 1 || suite.aead !== 2) throw new EpochKeyError("manifest_invalid", `epoch ${epoch}: suite`);
    const publicRaw = b64urlDecode(claims.pk, "pk");
    const wrapped = record(wrap.wrapped_private_key, "wrapped_private_key");
    let pair: CryptoKeyPair;
    try {
      pair = await openEpochWrap(
        identity.encryption,
        { enc: str(wrapped.enc, "enc"), ct: str(wrapped.ct, "ct") },
        { accountId: identity.accountId, feedGeneration, epoch, deviceId: identity.deviceId },
        publicRaw,
      );
    } catch (error) {
      if (error instanceof InboxCryptoError) throw new EpochKeyError("unwrap_failed", `epoch ${epoch}: ${error.reason}`);
      throw error;
    }
    keys.push({
      accountId: identity.accountId,
      feedGeneration,
      epoch,
      status: typeof claims.status === "string" ? claims.status : "unknown",
      publicRaw,
      pair,
    });
  }
  try {
    await store.saveEpochKeys(identity.accountId, feedGeneration, policyVersion, keys);
  } catch (error) {
    if (error instanceof Error && error.message === "policy_version_regression") throw new EpochKeyError("policy_regression", policyVersion);
    throw error;
  }
  const unavailable = list(response.unavailable_epochs ?? [], "unavailable_epochs").map((entry) => decimal(record(entry, "unavailable").epoch, "epoch"));
  return { epochs: keys.map((key) => key.epoch), unavailable, activeEpoch, feedGeneration };
}
