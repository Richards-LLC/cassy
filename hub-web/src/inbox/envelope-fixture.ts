// Deterministic builder of the cas-src envelope interop fixture (cas-9b7d S0).
//
// crates/cas-operator-crypto/tests/fixtures/cas-operator-envelope-interop.json
// is written by `UPDATE_OPERATOR_ENVELOPE_FIXTURE=1 npx vitest run
// src/inbox/hpke.test.ts`. Every key, ephemeral key, content key and nonce is
// derived from a fixed label, so the Rust test reseals the same inputs with the
// `hpke` crate and must reproduce every byte; vitest asserts the committed file
// equals a fresh build.

import {
  COMMAND_INFO,
  EVENT_KEY_INFO,
  commandAad,
  deriveKeyPair,
  eventAad,
  exportPublicKey,
  operatorSuite,
  sealCommand,
  sealEvent,
  type CommandIds,
  type EventIds,
} from "./hpke";

const encoder = new TextEncoder();

async function label(name: string): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", encoder.encode(`cas-op-envelope-fixture|${name}`)));
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
}

export const ENVELOPE_FIXTURE_EVENT_IDS: EventIds = {
  accountId: "acct-fixture-0001",
  feedGeneration: "1",
  keyEpoch: "7",
  eventId: "0f1e2d3c4b5a69788796a5b4c3d2e1f0",
  hubId: "hub-fixture-0001",
  projectId: "proj-fixture-0001",
  sessionId: "s_Xq9vH2c1d3Q8Kp0tR7mZbLwYfNa4UeJs6GhVoI5xCk8",
};

export const ENVELOPE_FIXTURE_COMMAND_IDS: CommandIds = {
  accountId: "acct-fixture-0001",
  machineId: "4f1c0a52-5d1e-4b7a-9c43-2a7e0f6b1d01",
  commandId: "cmdFixture0001AbCdEfGhIjKl",
  hubId: "hub-fixture-0001",
  projectId: "proj-fixture-0001",
  sessionId: "s_Xq9vH2c1d3Q8Kp0tR7mZbLwYfNa4UeJs6GhVoI5xCk8",
  operation: "operator_message",
  machineKeyId: "kid-fixture-cmd-0001",
};

// Plaintext shapes are owned by the S2/S4 slices; the crypto layer treats them
// as opaque bytes. Non-ASCII content proves UTF-8 bytes round-trip unchanged.
const EVENT_PLAINTEXT = JSON.stringify({
  type: "cas.operator.turn",
  v: 1,
  session_name: "cas-src-quiet hawk ✦",
  role: "supervisor",
  body: "Merged cas-4c78 — two devices replay independently.",
});

const COMMAND_PLAINTEXT = JSON.stringify({
  type: "cas.operator.command",
  v: 1,
  operation: "operator_message",
  body: "Ship it once soundwave is back — gracias.",
});

export async function buildEnvelopeFixture() {
  const suite = operatorSuite();

  const epochIkm = await label("epoch-7");
  const epoch = await deriveKeyPair(epochIkm);
  const eventRandomness = {
    contentKey: await label("event-content-key"),
    nonce: (await label("event-nonce")).subarray(0, 12),
    ikmEphemeral: await label("event-ephemeral"),
  };
  const eventPlain = encoder.encode(EVENT_PLAINTEXT);
  const event = await sealEvent(epoch.publicKey, eventPlain, ENVELOPE_FIXTURE_EVENT_IDS, eventRandomness);

  const machineIkm = await label("machine-command-key");
  const machine = await deriveKeyPair(machineIkm);
  const commandEphemeral = await label("command-ephemeral");
  const commandPlain = encoder.encode(COMMAND_PLAINTEXT);
  const command = await sealCommand(machine.publicKey, commandPlain, ENVELOPE_FIXTURE_COMMAND_IDS, commandEphemeral);

  return {
    fixture: "cas-operator-envelope-interop",
    version: 1,
    design: "cas-9b7d DESIGN.md D3 (cas-op-event-v1) and D4 (cas-op-command-v1)",
    generator: "hub-web/src/inbox/envelope-fixture.ts (@hpke/core 1.9.0); Rust must reproduce every byte",
    suite: "DHKEM(P-256, HKDF-SHA256) / HKDF-SHA256 / AES-256-GCM, base mode",
    encoding: "*_hex lowercase hex; sk 32-byte big-endian scalar; pk 65-byte uncompressed SEC1 point",
    event: {
      ids: ENVELOPE_FIXTURE_EVENT_IDS,
      key_info_utf8: EVENT_KEY_INFO,
      aad_utf8: eventAad(ENVELOPE_FIXTURE_EVENT_IDS),
      ikm_epoch_hex: hex(epochIkm),
      sk_epoch_hex: hex(new Uint8Array(await suite.kem.serializePrivateKey(epoch.privateKey))),
      pk_epoch_hex: hex(await exportPublicKey(epoch.publicKey)),
      content_key_hex: hex(eventRandomness.contentKey),
      nonce_hex: hex(eventRandomness.nonce),
      ikm_ephemeral_hex: hex(eventRandomness.ikmEphemeral),
      plaintext_utf8: EVENT_PLAINTEXT,
      envelope_utf8: new TextDecoder().decode(event.bytes),
      digest: event.digest,
    },
    command: {
      ids: ENVELOPE_FIXTURE_COMMAND_IDS,
      info_utf8: COMMAND_INFO,
      aad_utf8: commandAad(ENVELOPE_FIXTURE_COMMAND_IDS),
      ikm_machine_hex: hex(machineIkm),
      sk_machine_hex: hex(new Uint8Array(await suite.kem.serializePrivateKey(machine.privateKey))),
      pk_machine_hex: hex(await exportPublicKey(machine.publicKey)),
      ikm_ephemeral_hex: hex(commandEphemeral),
      plaintext_utf8: COMMAND_PLAINTEXT,
      envelope_utf8: new TextDecoder().decode(command.bytes),
      digest: command.digest,
    },
  };
}
