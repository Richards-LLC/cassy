// HPKE envelopes for the Commander operator inbox (cloud wire v1, cas-9b7d).
//
// The browser mirror of crates/cas-operator-crypto: the same suite
// (DHKEM(P-256, HKDF-SHA256) / HKDF-SHA256 / AES-256-GCM, base mode), the same
// info and AAD strings, and the same JSON envelope bytes. Both sides are
// pinned to the committed fixtures in crates/cas-operator-crypto/tests/fixtures.
//
// | Format | Owner | Where |
// | --- | --- | --- |
// | Epoch wrap (epoch private scalar → device) | cloud | contract §6.4 |
// | Enrollment key check | cloud | contract §5.1 |
// | Observer envelope v1 (psc-op-observer-v1) | cloud | contract §7.4 |
// | Session event envelope v1 (cas-op-event-v1) | cas-src | cas-9b7d DESIGN D3 |
// | Command envelope v1 (cas-op-command-v1) | cas-src | cas-9b7d DESIGN D4 |
//
// Keys cross this module as CryptoKeys inside the browser; raw bytes appear
// only at the wire (a 32-byte big-endian scalar or a 65-byte uncompressed
// point).

import { Aes256Gcm, CipherSuite, DhkemP256HkdfSha256, HkdfSha256 } from "@hpke/core";

export const EPOCH_WRAP_INFO = "psc-op-epoch-wrap-v1";
export const ENROLLMENT_CHECK_INFO = "psc-op-enc-check-v1";

export const OBSERVER_ALG = "psc-op-observer-v1";
export const OBSERVER_KEY_INFO = "psc-op-observer-key-v1";
export const OBSERVER_CONTENT_TYPE = "application/vnd.psc.operator.machine-presence+json; v=1";

export const EVENT_ALG = "cas-op-event-v1";
export const EVENT_KEY_INFO = "cas-op-event-key-v1";
export const EVENT_CONTENT_TYPE = "application/vnd.cas.operator.turn+json; v=1";

export const COMMAND_ALG = "cas-op-command-v1";
export const COMMAND_INFO = "cas-op-command-v1";

export const MAX_ENVELOPE_BYTES = 65_536;
const CONTENT_KEY_LEN = 32;
const NONCE_LEN = 12;

export type CryptoFailure =
  | "malformed"
  | "unsupported"
  | "epoch_mismatch"
  | "too_large"
  | "invalid_key"
  | "open_failed";

/** A refusal with a closed reason; never carries key or plaintext bytes. */
export class InboxCryptoError extends Error {
  constructor(readonly reason: CryptoFailure, detail: string) {
    super(`${reason}: ${detail}`);
    this.name = "InboxCryptoError";
  }
}

const encoder = new TextEncoder();

let cachedSuite: CipherSuite | null = null;
export function operatorSuite(): CipherSuite {
  cachedSuite ??= new CipherSuite({
    kem: new DhkemP256HkdfSha256(),
    kdf: new HkdfSha256(),
    aead: new Aes256Gcm(),
  });
  return cachedSuite;
}

// ------------------------------------------------------------- encoding

export function b64urlEncode(bytes: ArrayBuffer | Uint8Array): string {
  const input = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let binary = "";
  for (const value of input) binary += String.fromCharCode(value);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

export function b64urlDecode(text: string, what: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]*$/.test(text) || text.length % 4 === 1) {
    throw new InboxCryptoError("malformed", what);
  }
  const padded = text.replaceAll("-", "+").replaceAll("_", "/") + "=".repeat((4 - (text.length % 4)) % 4);
  const binary = atob(padded);
  const out = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) out[index] = binary.charCodeAt(index);
  return out;
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
}

/** The wire digest: `sha256:` + 64 lowercase hex (contract §4.3). */
export async function digest(bytes: Uint8Array): Promise<string> {
  return `sha256:${hex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as BufferSource)))}`;
}

// ------------------------------------------------------------- keys

export async function generateKeyPair(): Promise<CryptoKeyPair> {
  return operatorSuite().kem.generateKeyPair();
}

/** RFC 9180 DeriveKeyPair, for deterministic fixtures only. */
export async function deriveKeyPair(ikm: Uint8Array): Promise<CryptoKeyPair> {
  return operatorSuite().kem.deriveKeyPair(ikm);
}

export async function importPublicKey(raw: Uint8Array): Promise<CryptoKey> {
  if (raw.length !== 65) throw new InboxCryptoError("invalid_key", "public key length");
  try {
    return await operatorSuite().kem.deserializePublicKey(raw);
  } catch {
    throw new InboxCryptoError("invalid_key", "public key");
  }
}

export async function importPrivateKey(raw: Uint8Array): Promise<CryptoKey> {
  if (raw.length !== 32) throw new InboxCryptoError("invalid_key", "private key length");
  try {
    return await operatorSuite().kem.deserializePrivateKey(raw);
  } catch {
    throw new InboxCryptoError("invalid_key", "private key");
  }
}

export async function exportPublicKey(key: CryptoKey): Promise<Uint8Array> {
  return new Uint8Array(await operatorSuite().kem.serializePublicKey(key));
}

// ------------------------------------------------------------- HPKE core

export interface HpkeSealed {
  enc: Uint8Array;
  ciphertext: Uint8Array;
}

/**
 * HPKE base-mode single-shot seal. `ikmEphemeral` pins the ephemeral key for
 * fixtures (the Rust side feeds the same 32 bytes to its sender); production
 * callers omit it.
 */
export async function seal(
  recipient: CryptoKey,
  info: string,
  plaintext: Uint8Array,
  aad: string,
  ikmEphemeral?: Uint8Array,
): Promise<HpkeSealed> {
  const suite = operatorSuite();
  const sender = await suite.createSenderContext({
    recipientPublicKey: recipient,
    info: encoder.encode(info),
    ...(ikmEphemeral ? { ekm: await suite.kem.deriveKeyPair(ikmEphemeral) } : {}),
  });
  const ciphertext = new Uint8Array(await sender.seal(plaintext, encoder.encode(aad)));
  return { enc: new Uint8Array(sender.enc), ciphertext };
}

export async function open(
  recipient: CryptoKey,
  enc: Uint8Array,
  info: string,
  ciphertext: Uint8Array,
  aad: string,
): Promise<Uint8Array> {
  try {
    const plain = await operatorSuite().open(
      { recipientKey: recipient, enc, info: encoder.encode(info) },
      ciphertext,
      encoder.encode(aad),
    );
    return new Uint8Array(plain);
  } catch {
    throw new InboxCryptoError("open_failed", "hpke");
  }
}

async function aesKey(raw: Uint8Array, usage: KeyUsage): Promise<CryptoKey> {
  if (raw.length !== CONTENT_KEY_LEN) throw new InboxCryptoError("malformed", "content key");
  return crypto.subtle.importKey("raw", raw as BufferSource, { name: "AES-GCM" }, false, [usage]);
}

async function aesGcmSeal(key: Uint8Array, nonce: Uint8Array, plaintext: Uint8Array, aad: string) {
  const sealed = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv: nonce as BufferSource, additionalData: encoder.encode(aad), tagLength: 128 },
    await aesKey(key, "encrypt"),
    plaintext as BufferSource,
  );
  return new Uint8Array(sealed);
}

async function aesGcmOpen(key: Uint8Array, nonce: Uint8Array, ciphertext: Uint8Array, aad: string) {
  const cryptoKey = await aesKey(key, "decrypt");
  try {
    const plain = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: nonce as BufferSource, additionalData: encoder.encode(aad), tagLength: 128 },
      cryptoKey,
      ciphertext as BufferSource,
    );
    return new Uint8Array(plain);
  } catch {
    throw new InboxCryptoError("open_failed", "aes-gcm");
  }
}

// ------------------------------------------------------------- cloud §6.4, §5.1

export interface EpochWrapIds {
  accountId: string;
  feedGeneration: string;
  epoch: string;
  deviceId: string;
}

export function epochWrapAad(ids: EpochWrapIds): string {
  return `${ids.accountId}|${ids.feedGeneration}|${ids.epoch}|${ids.deviceId}`;
}

/**
 * Open a `/keys/wraps` entry with the device encryption key. Returns the epoch
 * private key as a CryptoKey; the raw scalar never leaves this function.
 */
export async function openEpochWrap(
  deviceKey: CryptoKey,
  wrap: { enc: string; ct: string },
  ids: EpochWrapIds,
): Promise<CryptoKey> {
  const scalar = await open(
    deviceKey,
    b64urlDecode(wrap.enc, "enc"),
    EPOCH_WRAP_INFO,
    b64urlDecode(wrap.ct, "ct"),
    epochWrapAad(ids),
  );
  try {
    return await importPrivateKey(scalar);
  } finally {
    scalar.fill(0);
  }
}

/** Open the enrollment `encryption_key_check`; returns base64url plaintext. */
export async function openEnrollmentCheck(
  deviceKey: CryptoKey,
  check: { enc: string; ct: string },
  enrollmentId: string,
): Promise<string> {
  const plain = await open(
    deviceKey,
    b64urlDecode(check.enc, "enc"),
    ENROLLMENT_CHECK_INFO,
    b64urlDecode(check.ct, "ct"),
    enrollmentId,
  );
  return b64urlEncode(plain);
}

// ------------------------------------------------------------- envelopes

interface WrappedEnvelope {
  v: 1;
  alg: string;
  epoch: string;
  enc: string;
  wk: string;
  n: string;
  ct: string;
}

export interface SealedEnvelope {
  bytes: Uint8Array;
  digest: string;
}

interface WrappedSpec {
  alg: string;
  keyInfo: string;
  epoch: string;
  aad: string;
}

export interface WrappedRandomness {
  contentKey: Uint8Array;
  nonce: Uint8Array;
  ikmEphemeral?: Uint8Array;
}

function freshRandomness(): WrappedRandomness {
  return {
    contentKey: crypto.getRandomValues(new Uint8Array(CONTENT_KEY_LEN)),
    nonce: crypto.getRandomValues(new Uint8Array(NONCE_LEN)),
  };
}

async function sealWrapped(
  spec: WrappedSpec,
  epochPublic: CryptoKey,
  plaintext: Uint8Array,
  randomness: WrappedRandomness,
): Promise<SealedEnvelope> {
  if (randomness.nonce.length !== NONCE_LEN) throw new InboxCryptoError("malformed", "nonce");
  const wrapped = await seal(epochPublic, spec.keyInfo, randomness.contentKey, spec.aad, randomness.ikmEphemeral);
  const ct = await aesGcmSeal(randomness.contentKey, randomness.nonce, plaintext, spec.aad);
  // Literal key order is the wire order (matches serde field order in Rust).
  const envelope: WrappedEnvelope = {
    v: 1,
    alg: spec.alg,
    epoch: spec.epoch,
    enc: b64urlEncode(wrapped.enc),
    wk: b64urlEncode(wrapped.ciphertext),
    n: b64urlEncode(randomness.nonce),
    ct: b64urlEncode(ct),
  };
  const bytes = encoder.encode(JSON.stringify(envelope));
  if (bytes.length > MAX_ENVELOPE_BYTES) throw new InboxCryptoError("too_large", "envelope");
  return { bytes, digest: await digest(bytes) };
}

function parseJson(bytes: Uint8Array): Record<string, unknown> {
  if (bytes.length > MAX_ENVELOPE_BYTES) throw new InboxCryptoError("too_large", "envelope");
  let value: unknown;
  try {
    value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    throw new InboxCryptoError("malformed", "envelope");
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new InboxCryptoError("malformed", "envelope");
  }
  return value as Record<string, unknown>;
}

function stringField(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  if (typeof value !== "string") throw new InboxCryptoError("malformed", key);
  return value;
}

async function openWrapped(spec: WrappedSpec, epochSecret: CryptoKey, envelope: Uint8Array): Promise<Uint8Array> {
  const parsed = parseJson(envelope);
  if (parsed.v !== 1 || parsed.alg !== spec.alg) throw new InboxCryptoError("unsupported", "envelope");
  const epoch = stringField(parsed, "epoch");
  if (epoch !== spec.epoch) throw new InboxCryptoError("epoch_mismatch", `${epoch} != ${spec.epoch}`);
  const contentKey = await open(
    epochSecret,
    b64urlDecode(stringField(parsed, "enc"), "enc"),
    spec.keyInfo,
    b64urlDecode(stringField(parsed, "wk"), "wk"),
    spec.aad,
  );
  try {
    if (contentKey.length !== CONTENT_KEY_LEN) throw new InboxCryptoError("malformed", "content key");
    const nonce = b64urlDecode(stringField(parsed, "n"), "n");
    if (nonce.length !== NONCE_LEN) throw new InboxCryptoError("malformed", "n");
    return await aesGcmOpen(contentKey, nonce, b64urlDecode(stringField(parsed, "ct"), "ct"), spec.aad);
  } finally {
    contentKey.fill(0);
  }
}

// --------------------------------------------- observer envelope (§7.4)

export interface ObserverIds {
  accountId: string;
  feedGeneration: string;
  keyEpoch: string;
  eventId: string;
  hubId: string;
}

export function observerAad(ids: ObserverIds): string {
  return `${OBSERVER_ALG}|${ids.accountId}|${ids.feedGeneration}|${ids.keyEpoch}|${ids.eventId}|${ids.hubId}|${OBSERVER_CONTENT_TYPE}`;
}

function observerSpec(ids: ObserverIds): WrappedSpec {
  return { alg: OBSERVER_ALG, keyInfo: OBSERVER_KEY_INFO, epoch: ids.keyEpoch, aad: observerAad(ids) };
}

/** Open a cloud observer notice; the §7.4 assertion checks remain the caller's. */
export async function openObserverNotice(epochSecret: CryptoKey, envelope: Uint8Array, ids: ObserverIds) {
  return openWrapped(observerSpec(ids), epochSecret, envelope);
}

/** Reproduce a cloud observer notice byte-for-byte (fixture parity only). */
export async function sealObserverNoticeWith(
  epochPublic: CryptoKey,
  plaintext: Uint8Array,
  ids: ObserverIds,
  randomness: WrappedRandomness,
) {
  return sealWrapped(observerSpec(ids), epochPublic, plaintext, randomness);
}

// --------------------------------------------- session event envelope (D3)

export interface EventIds {
  accountId: string;
  feedGeneration: string;
  keyEpoch: string;
  eventId: string;
  hubId: string;
  projectId: string;
  sessionId: string;
}

export function eventAad(ids: EventIds): string {
  return [
    EVENT_ALG,
    ids.accountId,
    ids.feedGeneration,
    ids.keyEpoch,
    ids.eventId,
    ids.hubId,
    ids.projectId,
    ids.sessionId,
    EVENT_CONTENT_TYPE,
  ].join("|");
}

function eventSpec(ids: EventIds): WrappedSpec {
  return { alg: EVENT_ALG, keyInfo: EVENT_KEY_INFO, epoch: ids.keyEpoch, aad: eventAad(ids) };
}

export async function sealEvent(
  epochPublic: CryptoKey,
  plaintext: Uint8Array,
  ids: EventIds,
  randomness: WrappedRandomness = freshRandomness(),
): Promise<SealedEnvelope> {
  return sealWrapped(eventSpec(ids), epochPublic, plaintext, randomness);
}

export async function openEvent(epochSecret: CryptoKey, envelope: Uint8Array, ids: EventIds) {
  return openWrapped(eventSpec(ids), epochSecret, envelope);
}

// --------------------------------------------- command envelope (D4)

export interface CommandIds {
  accountId: string;
  machineId: string;
  commandId: string;
  hubId: string;
  projectId: string;
  sessionId: string;
  operation: string;
  machineKeyId: string;
}

export function commandAad(ids: CommandIds): string {
  return [
    COMMAND_ALG,
    ids.accountId,
    ids.machineId,
    ids.commandId,
    ids.hubId,
    ids.projectId,
    ids.sessionId,
    ids.operation,
    ids.machineKeyId,
  ].join("|");
}

export async function sealCommand(
  machinePublic: CryptoKey,
  plaintext: Uint8Array,
  ids: CommandIds,
  ikmEphemeral?: Uint8Array,
): Promise<SealedEnvelope> {
  const sealed = await seal(machinePublic, COMMAND_INFO, plaintext, commandAad(ids), ikmEphemeral);
  const envelope = {
    v: 1,
    alg: COMMAND_ALG,
    kid: ids.machineKeyId,
    enc: b64urlEncode(sealed.enc),
    ct: b64urlEncode(sealed.ciphertext),
  };
  const bytes = encoder.encode(JSON.stringify(envelope));
  if (bytes.length > MAX_ENVELOPE_BYTES) throw new InboxCryptoError("too_large", "envelope");
  return { bytes, digest: await digest(bytes) };
}

export async function openCommand(machineSecret: CryptoKey, envelope: Uint8Array, ids: CommandIds) {
  const parsed = parseJson(envelope);
  if (parsed.v !== 1 || parsed.alg !== COMMAND_ALG) throw new InboxCryptoError("unsupported", "envelope");
  if (stringField(parsed, "kid") !== ids.machineKeyId) throw new InboxCryptoError("malformed", "kid");
  return open(
    machineSecret,
    b64urlDecode(stringField(parsed, "enc"), "enc"),
    COMMAND_INFO,
    b64urlDecode(stringField(parsed, "ct"), "ct"),
    commandAad(ids),
  );
}
