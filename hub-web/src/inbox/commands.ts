// Offline operator messages, device side (cloud contract §10.1, §10.5).
//
// While the machine is off, a reply becomes a structured `operator_message`
// command: the cloud holds it as "Pending machine" for 24 hours until that
// machine reserves it, admits it once and sends a receipt.
//
// 1. The command ID is written to this profile's IndexedDB before anything
//    is sealed, so a reload or a retry can never mint a second command.
// 2. The machine binding is verified (issuer signature, account, machine,
//    hub) before its command key is trusted; the device's own grant must
//    cover the exact hub/project/session.
// 3. The body is sealed twice: to the machine's command key (D4) and, as a
//    history event every other device replays, to the active epoch key (D3).
//    The exact request is stored, and every retry sends those bytes.
// 4. Only a machine receipt `accepted` reads "Accepted by machine".

import { b64urlDecode, b64urlEncode, importPublicKey, sealCommand, sealEvent } from "./hpke";
import { ISSUER_TYP, type IssuerKeys } from "./issuer";
import type { CommandState, InboxIdentity, InboxStore, QueuedCommand } from "./store";
import { OperatorWireError, record, str, type OperatorClient } from "./wire";

export const OPERATION = "operator_message";
const MAX_BODY_BYTES = 32 * 1024;

export interface CommandContext {
  client: OperatorClient;
  store: InboxStore;
  issuer: IssuerKeys;
  identity: InboxIdentity;
  /** Re-fetch epoch keys (after `epoch_retired`). */
  refreshKeys: () => Promise<void>;
}

export interface CommandTarget {
  machineId: string;
  hubId: string;
  projectId: string;
  /** Opaque session routing ID (`s_` + base64url SHA-256 of the name). */
  sessionId: string;
  /** Readable session name; travels only inside ciphertext. */
  sessionName: string;
}

export class CommandRefusal extends Error {
  constructor(readonly reason: string) {
    super(`offline message refused: ${reason}`);
    this.name = "CommandRefusal";
  }
}

const encoder = new TextEncoder();

/** The session routing ID the hub derives for a session name (DESIGN D7). */
export async function sessionRoutingId(sessionName: string): Promise<string> {
  return `s_${b64urlEncode(await crypto.subtle.digest("SHA-256", encoder.encode(sessionName)))}`;
}

/**
 * The reply's state in the operator's words, naming the machine it waits on
 * (cas-97d58 F21): "Waiting for soundwave", then "soundwave received it".
 */
export function commandStatusLabel(state: CommandState, machine = "the machine"): string {
  switch (state) {
    case "sealing":
    case "submitting":
      return "Queued on this device";
    case "pending_machine":
    case "reserved":
      return `Waiting for ${machine}`;
    case "accepted":
      return `${machine === "the machine" ? "The machine" : machine} received it`;
    case "rejected_by_machine":
      return `${machine === "the machine" ? "The machine" : machine} refused it`;
    case "cancelled":
      return "Cancelled";
    case "expired":
      return `Expired before ${machine} came back`;
    case "refused":
      return "Not sent";
  }
}

const TERMINAL: ReadonlySet<CommandState> = new Set(["accepted", "rejected_by_machine", "cancelled", "expired", "refused"]);

function scopeCovers(identity: InboxIdentity, target: CommandTarget): boolean {
  return identity.grant.scopes.some(
    (scope) =>
      scope.hub_id === target.hubId &&
      scope.project_id === target.projectId &&
      (scope.session_id === null || scope.session_id === target.sessionId) &&
      scope.operations.includes(OPERATION),
  );
}

interface VerifiedBinding {
  commandPublic: CryptoKey;
  commandKeyId: string;
}

async function verifiedBinding(context: CommandContext, target: CommandTarget): Promise<VerifiedBinding> {
  const principals = await context.client.principals();
  const machine = (Array.isArray(principals.machines) ? principals.machines : [])
    .map((entry) => record(entry, "machine"))
    .find((entry) => entry.machine_id === target.machineId);
  if (!machine || machine.status !== "active" || typeof machine.machine_binding !== "string") {
    throw new CommandRefusal("machine_unavailable");
  }
  let claims: Record<string, unknown>;
  try {
    ({ claims } = await context.issuer.verify(machine.machine_binding, ISSUER_TYP.machineBinding));
  } catch {
    throw new CommandRefusal("machine_binding_invalid");
  }
  if (claims.acct !== context.identity.accountId || claims.mch !== target.machineId || claims.hub !== target.hubId) {
    throw new CommandRefusal("machine_binding_mismatch");
  }
  const projects = Array.isArray(claims.projects) ? claims.projects : [];
  if (!projects.includes(target.projectId)) throw new CommandRefusal("project_not_on_machine");
  return {
    commandPublic: await importPublicKey(b64urlDecode(str(claims.cmd_pk, "cmd_pk"), "cmd_pk")),
    commandKeyId: str(claims.cmd_kid, "cmd_kid"),
  };
}

async function activeEpochKey(context: CommandContext) {
  const keys = await context.client.keyWraps().catch(() => null);
  const active = keys && typeof keys.active_epoch === "string" ? keys.active_epoch : null;
  if (!active) throw new CommandRefusal("epoch_unavailable");
  let key = await context.store.epochKey(context.identity.accountId, context.identity.feedGeneration, active);
  if (!key) {
    await context.refreshKeys();
    key = await context.store.epochKey(context.identity.accountId, context.identity.feedGeneration, active);
  }
  if (!key) throw new CommandRefusal("epoch_unavailable");
  return key;
}

async function sealRequest(context: CommandContext, command: QueuedCommand, target: CommandTarget): Promise<Record<string, unknown>> {
  const binding = await verifiedBinding(context, target);
  const epoch = await activeEpochKey(context);
  const plaintext = encoder.encode(
    JSON.stringify({ type: "cas.operator.command", v: 1, operation: OPERATION, session_name: target.sessionName, body: command.body }),
  );
  const machine = await sealCommand(binding.commandPublic, plaintext, {
    accountId: context.identity.accountId,
    machineId: target.machineId,
    commandId: command.commandId,
    hubId: target.hubId,
    projectId: target.projectId,
    sessionId: target.sessionId,
    operation: OPERATION,
    machineKeyId: binding.commandKeyId,
  });
  const history = await sealEvent(
    epoch.pair.publicKey,
    encoder.encode(
      JSON.stringify({
        type: "cas.operator.turn",
        v: 1,
        event_id: command.historyEventId,
        session_name: target.sessionName,
        snapshot: {
          source: `commander:${context.identity.deviceId}`,
          target: "supervisor",
          prompt: command.body,
          created_at: command.createdAt,
          factory_session: target.sessionName,
          kind: OPERATION,
          command_id: command.commandId,
        },
      }),
    ),
    {
      accountId: context.identity.accountId,
      feedGeneration: context.identity.feedGeneration,
      keyEpoch: epoch.epoch,
      eventId: command.historyEventId,
      hubId: target.hubId,
      projectId: target.projectId,
      sessionId: target.sessionId,
    },
  );
  return {
    command_id: command.commandId,
    machine_id: target.machineId,
    hub_id: target.hubId,
    project_id: target.projectId,
    session_id: target.sessionId,
    operation: OPERATION,
    machine_key_id: binding.commandKeyId,
    machine_ciphertext: b64urlEncode(machine.bytes),
    machine_digest: machine.digest,
    history_event: {
      event_id: command.historyEventId,
      key_epoch: epoch.epoch,
      ciphertext: b64urlEncode(history.bytes),
      digest: history.digest,
    },
  };
}

function statusFrom(value: unknown): CommandState | null {
  const known: CommandState[] = ["pending_machine", "reserved", "accepted", "rejected_by_machine", "cancelled", "expired"];
  return known.includes(value as CommandState) ? (value as CommandState) : null;
}

async function save(context: CommandContext, command: QueuedCommand, patch: Partial<QueuedCommand>): Promise<QueuedCommand> {
  const next = { ...command, ...patch, updatedAt: new Date().toISOString() };
  await context.store.saveCommand(next);
  return next;
}

/** Submit (or re-submit) one stored command; never mints a new ID. */
async function submit(context: CommandContext, command: QueuedCommand, target: CommandTarget): Promise<QueuedCommand> {
  let current = command;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    if (!current.request) current = await save(context, current, { request: await sealRequest(context, current, target), state: "submitting" });
    try {
      const response = await context.client.submitCommand(current.request!);
      return save(context, current, { state: statusFrom(response.status) ?? "pending_machine", reason: null });
    } catch (error) {
      if (!(error instanceof OperatorWireError)) throw error;
      if ((error.code === "epoch_retired" || error.code === "machine_key_stale") && attempt === 0) {
        // Nothing was stored (§10.1): re-seal under the current keys, same IDs.
        if (error.code === "epoch_retired") await context.refreshKeys();
        current = await save(context, current, { request: null, state: "sealing" });
        continue;
      }
      if (error.retryable) return save(context, current, { state: "submitting", reason: error.code });
      return save(context, current, { state: "refused", reason: error.code });
    }
  }
  return current;
}

export async function queueOfflineMessage(context: CommandContext, target: CommandTarget, body: string): Promise<QueuedCommand> {
  if (!body.trim() || encoder.encode(body).length > MAX_BODY_BYTES) throw new CommandRefusal("body_invalid");
  if (!scopeCovers(context.identity, target)) throw new CommandRefusal("scope_not_granted");
  if ((await sessionRoutingId(target.sessionName)) !== target.sessionId) throw new CommandRefusal("session_mismatch");
  const now = new Date().toISOString();
  const command: QueuedCommand = {
    commandId: b64urlEncode(crypto.getRandomValues(new Uint8Array(16))),
    accountId: context.identity.accountId,
    machineId: target.machineId,
    hubId: target.hubId,
    projectId: target.projectId,
    sessionId: target.sessionId,
    sessionName: target.sessionName,
    request: null,
    body,
    historyEventId: b64urlEncode(crypto.getRandomValues(new Uint8Array(16))),
    state: "sealing",
    reason: null,
    createdAt: now,
    updatedAt: now,
  };
  // Durable identity first: everything after this reuses the same IDs.
  await context.store.saveCommand(command);
  return submit(context, command, { ...target });
}

/** Retry commands a crash or outage left on this device, then refresh statuses. */
export async function syncCommands(context: CommandContext): Promise<QueuedCommand[]> {
  const out: QueuedCommand[] = [];
  for (const command of await context.store.commands(context.identity.accountId)) {
    if (TERMINAL.has(command.state)) {
      out.push(command);
      continue;
    }
    if (command.state === "sealing" || command.state === "submitting") {
      out.push(
        await submit(context, command, {
          machineId: command.machineId,
          hubId: command.hubId,
          projectId: command.projectId,
          sessionId: command.sessionId,
          sessionName: command.sessionName,
        }),
      );
      continue;
    }
    try {
      const status = await context.client.commandStatus(command.commandId);
      const state = statusFrom(status.status);
      out.push(
        state && state !== command.state
          ? await save(context, command, { state, reason: typeof status.reason === "string" ? status.reason : null })
          : command,
      );
    } catch {
      out.push(command);
    }
  }
  return out;
}

export async function cancelOfflineMessage(context: CommandContext, commandId: string): Promise<QueuedCommand | null> {
  const command = (await context.store.commands(context.identity.accountId)).find((entry) => entry.commandId === commandId);
  if (!command) return null;
  try {
    await context.client.cancelCommand(commandId);
    return save(context, command, { state: "cancelled", reason: null });
  } catch (error) {
    if (!(error instanceof OperatorWireError)) throw error;
    if (error.code === "handoff_in_progress") return save(context, command, { state: "reserved", reason: "handoff_in_progress" });
    if (error.code === "command_expired") return save(context, command, { state: "expired", reason: null });
    if (error.code === "command_terminal") {
      const receipt = record(error.body.receipt ?? {}, "receipt");
      return save(context, command, { state: receipt.outcome === "rejected" ? "rejected_by_machine" : "accepted", reason: null });
    }
    throw error;
  }
}
