// cas-9b7d S0 crypto gate (browser side). The Rust side is
// crates/cas-operator-crypto/tests/interop.rs; both open the same committed
// fixtures and both must reproduce the sealed bytes exactly.

import { readFileSync, writeFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { buildEnvelopeFixture, ENVELOPE_FIXTURE_COMMAND_IDS, ENVELOPE_FIXTURE_EVENT_IDS } from "./envelope-fixture";
import {
  EPOCH_WRAP_INFO,
  InboxCryptoError,
  b64urlDecode,
  b64urlEncode,
  deriveKeyPair,
  digest,
  exportPublicKey,
  generateKeyPair,
  importPrivateKey,
  importPublicKey,
  open,
  openCommand,
  openEpochWrap,
  openEvent,
  openObserverNotice,
  operatorSuite,
  seal,
  sealCommand,
  sealEvent,
  sealObserverNoticeWith,
  type CommandIds,
  type EventIds,
  type ObserverIds,
} from "./hpke";

const FIXTURES = new URL("../../../crates/cas-operator-crypto/tests/fixtures/", import.meta.url);
const cloud = JSON.parse(readFileSync(new URL("cloud-operator-hpke-interop.json", FIXTURES), "utf8"));
const ENVELOPE_PATH = new URL("cas-operator-envelope-interop.json", FIXTURES);

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const fromHex = (text: string) => new Uint8Array(Buffer.from(text, "hex"));
const toHex = (bytes: Uint8Array | ArrayBuffer) =>
  Buffer.from(bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes)).toString("hex");

async function rejection(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
  } catch (error) {
    expect(error).toBeInstanceOf(InboxCryptoError);
    return (error as InboxCryptoError).reason;
  }
  throw new Error("expected a refusal");
}

function cloudCase(name: string) {
  const found = cloud.cases.find((entry: { name: string }) => entry.name === name);
  expect(found, name).toBeTruthy();
  return found;
}

describe("cloud fixture (petra-stella-cloud tests/fixtures/operator-hpke-interop.json)", () => {
  it("is the suite the contract pins", () => {
    expect(cloud.suite).toMatchObject({ kem_id: 16, kdf_id: 1, aead_id: 2, mode: 0 });
  });

  for (const name of ["epoch_wrap", "event_content_key"]) {
    it(`${name}: derives the recipient, opens the JS seal and reproduces it byte-exact`, async () => {
      const entry = cloudCase(name);
      const suite = operatorSuite();
      const recipient = await deriveKeyPair(fromHex(entry.ikm_recipient_hex));
      expect(toHex(await suite.kem.serializePrivateKey(recipient.privateKey))).toBe(entry.sk_recipient_hex);
      expect(toHex(await exportPublicKey(recipient.publicKey))).toBe(entry.pk_recipient_hex);

      const imported = await importPrivateKey(fromHex(entry.sk_recipient_hex));
      const plain = await open(imported, fromHex(entry.enc_hex), entry.info_utf8, fromHex(entry.ciphertext_hex), entry.aad_utf8);
      expect(toHex(plain)).toBe(entry.plaintext_hex);

      const resealed = await seal(
        await importPublicKey(fromHex(entry.pk_recipient_hex)),
        entry.info_utf8,
        fromHex(entry.plaintext_hex),
        entry.aad_utf8,
        fromHex(entry.ikm_ephemeral_hex),
      );
      expect(toHex(resealed.enc)).toBe(entry.enc_hex);
      expect(toHex(resealed.ciphertext)).toBe(entry.ciphertext_hex);
    });
  }

  it("walks the device path: device key → epoch wrap → epoch key → event content key", async () => {
    const wrap = cloudCase("epoch_wrap");
    const eventKey = cloudCase("event_content_key");
    expect(wrap.info_utf8).toBe(EPOCH_WRAP_INFO);
    const deviceKey = await importPrivateKey(fromHex(wrap.sk_recipient_hex));
    const ids = {
      accountId: cloud.ids.accountId,
      feedGeneration: cloud.ids.feedGeneration,
      epoch: cloud.ids.epoch,
      deviceId: cloud.ids.deviceId,
    };
    const epochPublic = fromHex(eventKey.pk_recipient_hex);
    const epochKey = await openEpochWrap(
      deviceKey,
      { enc: b64urlEncode(fromHex(wrap.enc_hex)), ct: b64urlEncode(fromHex(wrap.ciphertext_hex)) },
      ids,
      epochPublic,
    );
    expect(epochKey.privateKey.extractable).toBe(false);
    const contentKey = await open(epochKey, fromHex(eventKey.enc_hex), eventKey.info_utf8, fromHex(eventKey.ciphertext_hex), eventKey.aad_utf8);
    expect(toHex(contentKey)).toBe(eventKey.plaintext_hex);

    // A scalar paired with another epoch's public key can never open its events.
    const otherPublic = await exportPublicKey((await generateKeyPair()).publicKey);
    const mismatched = await openEpochWrap(
      deviceKey,
      { enc: b64urlEncode(fromHex(wrap.enc_hex)), ct: b64urlEncode(fromHex(wrap.ciphertext_hex)) },
      ids,
      otherPublic,
    );
    expect(
      await rejection(open(mismatched, fromHex(eventKey.enc_hex), eventKey.info_utf8, fromHex(eventKey.ciphertext_hex), eventKey.aad_utf8)),
    ).toBe("open_failed");

    // Every binding of the wrap is load-bearing.
    for (const field of ["accountId", "feedGeneration", "epoch", "deviceId"] as const) {
      const tampered = { ...ids, [field]: `${ids[field]}x` };
      expect(
        await rejection(
          openEpochWrap(deviceKey, { enc: b64urlEncode(fromHex(wrap.enc_hex)), ct: b64urlEncode(fromHex(wrap.ciphertext_hex)) }, tampered, epochPublic),
        ),
      ).toBe("open_failed");
    }
  });

  describe("observer_notices[0] (contract §7.4, §16 Q11 gate)", () => {
    const notice = cloud.observer_notices[0];
    const ids: ObserverIds = {
      accountId: notice.ids.accountId,
      feedGeneration: notice.ids.feedGeneration,
      keyEpoch: notice.ids.epoch,
      eventId: notice.ids.eventId,
      hubId: notice.ids.hubId,
    };
    const epochSecret = () => importPrivateKey(fromHex(cloudCase("event_content_key").sk_recipient_hex));

    it("opens the cloud-sealed notice and matches its digest", async () => {
      const envelope = encoder.encode(notice.envelope_utf8);
      expect(await digest(envelope)).toBe(notice.digest);
      const plain = await openObserverNotice(await epochSecret(), envelope, ids);
      expect(decoder.decode(plain)).toBe(notice.plaintext_utf8);
      expect(JSON.parse(decoder.decode(plain))).toMatchObject({
        type: "psc.operator.machine_presence",
        kind: "machine_unobserved",
        machine_id: notice.ids.machineId,
        hub_id: notice.ids.hubId,
      });
    });

    it("reproduces the cloud envelope bytes from the same inputs", async () => {
      const sealed = await sealObserverNoticeWith(
        await importPublicKey(fromHex(cloudCase("event_content_key").pk_recipient_hex)),
        encoder.encode(notice.plaintext_utf8),
        ids,
        {
          contentKey: fromHex(notice.content_key_hex),
          nonce: fromHex(notice.nonce_hex),
          ikmEphemeral: fromHex(notice.ikm_ephemeral_hex),
        },
      );
      expect(decoder.decode(sealed.bytes)).toBe(notice.envelope_utf8);
      expect(sealed.digest).toBe(notice.digest);
    });

    it("refuses a notice bound to any other row value", async () => {
      const envelope = encoder.encode(notice.envelope_utf8);
      for (const field of ["accountId", "feedGeneration", "eventId", "hubId"] as const) {
        expect(await rejection(openObserverNotice(await epochSecret(), envelope, { ...ids, [field]: `${ids[field]}x` }))).toBe(
          "open_failed",
        );
      }
      expect(await rejection(openObserverNotice(await epochSecret(), envelope, { ...ids, keyEpoch: "5" }))).toBe("epoch_mismatch");
    });
  });
});

describe("cas-src envelope fixture (DESIGN D3/D4)", () => {
  it("is reproduced byte-for-byte by a fresh build", async () => {
    const built = `${JSON.stringify(await buildEnvelopeFixture(), null, 2)}\n`;
    if (process.env.UPDATE_OPERATOR_ENVELOPE_FIXTURE === "1") writeFileSync(ENVELOPE_PATH, built);
    expect(readFileSync(ENVELOPE_PATH, "utf8")).toBe(built);
  });

  const fixture = () => JSON.parse(readFileSync(ENVELOPE_PATH, "utf8"));

  it("opens the session event and refuses every substituted binding", async () => {
    const { event } = fixture();
    const secret = await importPrivateKey(fromHex(event.sk_epoch_hex));
    const envelope = encoder.encode(event.envelope_utf8);
    expect(await digest(envelope)).toBe(event.digest);
    expect(decoder.decode(await openEvent(secret, envelope, ENVELOPE_FIXTURE_EVENT_IDS))).toBe(event.plaintext_utf8);

    for (const field of ["accountId", "feedGeneration", "eventId", "hubId", "projectId", "sessionId"] as const) {
      const tampered: EventIds = { ...ENVELOPE_FIXTURE_EVENT_IDS, [field]: `${ENVELOPE_FIXTURE_EVENT_IDS[field]}x` };
      expect(await rejection(openEvent(secret, envelope, tampered)), field).toBe("open_failed");
    }
    expect(await rejection(openEvent(secret, envelope, { ...ENVELOPE_FIXTURE_EVENT_IDS, keyEpoch: "8" }))).toBe("epoch_mismatch");

    const asObserver = encoder.encode(event.envelope_utf8.replace('"alg":"cas-op-event-v1"', '"alg":"psc-op-observer-v1"'));
    expect(await rejection(openEvent(secret, asObserver, ENVELOPE_FIXTURE_EVENT_IDS))).toBe("unsupported");

    const parsed = JSON.parse(event.envelope_utf8);
    const body = b64urlDecode(parsed.ct, "ct");
    body[0] ^= 1;
    const flipped = encoder.encode(JSON.stringify({ ...parsed, ct: b64urlEncode(body) }));
    expect(await rejection(openEvent(secret, flipped, ENVELOPE_FIXTURE_EVENT_IDS))).toBe("open_failed");

    const other = await generateKeyPair();
    expect(await rejection(openEvent(other, envelope, ENVELOPE_FIXTURE_EVENT_IDS))).toBe("open_failed");
  });

  it("opens the command and refuses every substituted binding", async () => {
    const { command } = fixture();
    const secret = await importPrivateKey(fromHex(command.sk_machine_hex));
    const envelope = encoder.encode(command.envelope_utf8);
    expect(await digest(envelope)).toBe(command.digest);
    expect(decoder.decode(await openCommand(secret, envelope, ENVELOPE_FIXTURE_COMMAND_IDS))).toBe(command.plaintext_utf8);

    for (const field of ["accountId", "machineId", "commandId", "hubId", "projectId", "sessionId", "operation"] as const) {
      const tampered: CommandIds = { ...ENVELOPE_FIXTURE_COMMAND_IDS, [field]: `${ENVELOPE_FIXTURE_COMMAND_IDS[field]}x` };
      expect(await rejection(openCommand(secret, envelope, tampered)), field).toBe("open_failed");
    }
    expect(await rejection(openCommand(secret, envelope, { ...ENVELOPE_FIXTURE_COMMAND_IDS, machineKeyId: "other" }))).toBe("malformed");
  });

  it("round-trips fresh randomness and never repeats a ciphertext", async () => {
    const epoch = await generateKeyPair();
    expect(epoch.privateKey.extractable).toBe(false);
    const plain = encoder.encode("hello ✦");
    const first = await sealEvent(epoch.publicKey, plain, ENVELOPE_FIXTURE_EVENT_IDS);
    const second = await sealEvent(epoch.publicKey, plain, ENVELOPE_FIXTURE_EVENT_IDS);
    expect(first.digest).not.toBe(second.digest);
    expect(decoder.decode(await openEvent(epoch, first.bytes, ENVELOPE_FIXTURE_EVENT_IDS))).toBe("hello ✦");

    const machine = await generateKeyPair();
    const command = await sealCommand(machine.publicKey, plain, ENVELOPE_FIXTURE_COMMAND_IDS);
    expect(decoder.decode(await openCommand(machine, command.bytes, ENVELOPE_FIXTURE_COMMAND_IDS))).toBe("hello ✦");
  });

  it("refuses malformed keys and oversized envelopes", async () => {
    expect(await rejection(importPublicKey(new Uint8Array(64)))).toBe("invalid_key");
    expect(await rejection(importPrivateKey(new Uint8Array(31)))).toBe("invalid_key");
    const epoch = await generateKeyPair();
    expect(await rejection(openEvent(epoch, new Uint8Array(65_537), ENVELOPE_FIXTURE_EVENT_IDS))).toBe("too_large");
    expect(await rejection(openEvent(epoch, encoder.encode("[]"), ENVELOPE_FIXTURE_EVENT_IDS))).toBe("malformed");
  });
});
