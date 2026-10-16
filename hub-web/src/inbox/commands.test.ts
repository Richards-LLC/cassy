// cas-9b7d S4 device side: offline operator messages against the contract
// double. The machine side (reserve, admit once, receipt) is Rust,
// cas-cli/src/hub/operator_inbox/commands.rs.

import { describe, expect, it, vi } from "vitest";
import { OperatorCloudDouble } from "../../test/operator-cloud-double";
import { cancelOfflineMessage, commandStatusLabel, queueOfflineMessage, sessionRoutingId, syncCommands, type CommandContext } from "./commands";
import { completeEnrollment, pollEnrollment, refreshEpochKeys, startEnrollment } from "./enrollment";
import { b64urlDecode, exportPublicKey, generateKeyPair, openCommand } from "./hpke";
import { IssuerKeys } from "./issuer";
import { replayRound } from "./replay";
import { MemoryInboxStore } from "./store";
import { OperatorClient } from "./wire";

const PAGE_ORIGIN = "https://hub.petrastella.io";
const SESSION_NAME = "cas-src-quiet hawk ✦";

async function device(cloud: OperatorCloudDouble, label: string, scopes: { hub_id: string; project_id: string; session_id: string | null; operations: string[] }[]) {
  const client = new OperatorClient({ origin: cloud.baseUrl, fetch: cloud.fetchFor(PAGE_ORIGIN) });
  const store = new MemoryInboxStore();
  const issuer = new IssuerKeys(() => cloud.jwks());
  const pending = await startEnrollment(client, store, { pageOrigin: PAGE_ORIGIN, label });
  await cloud.approve(pending.userCode, { scopes });
  await pollEnrollment(client, pending);
  const identity = await completeEnrollment(client, store, issuer, pending, null);
  const refreshKeys = async () => {
    await refreshEpochKeys(client, store, issuer, identity);
  };
  const context: CommandContext = { client, store, issuer, identity, refreshKeys };
  return { client, store, issuer, identity, context, refreshKeys };
}

async function setup() {
  const cloud = new OperatorCloudDouble();
  const machineKeys = await generateKeyPair();
  const machine = await cloud.enrollMachine("hub-soundwave", ["proj-cas"], await exportPublicKey(machineKeys.publicKey));
  const sessionId = await sessionRoutingId(SESSION_NAME);
  const target = { machineId: machine.id, hubId: "hub-soundwave", projectId: "proj-cas", sessionId, sessionName: SESSION_NAME };
  const scope = [{ hub_id: "hub-soundwave", project_id: "proj-cas", session_id: null, operations: ["operator_message"] }];
  return { cloud, machine, machineKeys, target, scope };
}

describe("offline operator messages (§10)", () => {
  it("concurrent replay keeps one sealed request and cannot replace Accepted with Not sent", async () => {
    const { cloud, machine, machineKeys, target, scope } = await setup();
    const phone = await device(cloud, "new phone", scope);
    const gate = () => {
      let release!: () => void;
      const wait = new Promise<void>(resolve => { release = resolve; });
      return { wait, release };
    };
    const sealing = gate(), continueSeal = gate(), syncRead = gate(), staleRead = gate(), continueSync = gate();
    const principals = phone.client.principals.bind(phone.client);
    vi.spyOn(phone.client, "principals").mockImplementationOnce(async () => {
      sealing.release(); await continueSeal.wait; return principals();
    });
    const queueing = queueOfflineMessage(phone.context, target, "Ship it once soundwave is back");
    await sealing.wait;
    const commands = phone.store.commands.bind(phone.store);
    vi.spyOn(phone.store, "commands").mockImplementationOnce(async account => {
      const rows = await commands(account);
      syncRead.release(); return rows;
    }).mockImplementationOnce(async account => {
      const rows = await commands(account);
      expect(rows[0].state).toBe("sealing");
      staleRead.release(); await continueSync.wait; return rows;
    });
    const replaying = syncCommands(phone.context);
    await syncRead.wait;
    const delayedSync = syncCommands(phone.context);
    await staleRead.wait;
    continueSeal.release();
    const [queued] = await Promise.all([queueing, replaying]);

    // The machine can open exactly this command with its own key and IDs.
    const stored = cloud.commands.get(queued.commandId)!;
    const plain = await openCommand(machineKeys, b64urlDecode(stored.machineCiphertext, "ct"), {
      accountId: cloud.accountId,
      machineId: machine.id,
      commandId: queued.commandId,
      hubId: target.hubId,
      projectId: target.projectId,
      sessionId: target.sessionId,
      operation: "operator_message",
      machineKeyId: machine.commandKeyId,
    });
    expect(JSON.parse(new TextDecoder().decode(plain))).toMatchObject({ body: "Ship it once soundwave is back", session_name: SESSION_NAME });

    // Every other device sees the sent message through the feed.
    const desktop = await device(cloud, "desktop", []);
    const machines = async () => ({ has: () => true });
    await replayRound({ client: desktop.client, store: desktop.store, issuer: desktop.issuer, identity: desktop.identity, machines, refreshKeys: desktop.refreshKeys });
    const [history] = await desktop.store.events(cloud.accountId, "1");
    expect(history.eventId).toBe(queued.historyEventId);
    expect(history.plaintext).toMatchObject({ type: "cas.operator.turn", snapshot: { prompt: "Ship it once soundwave is back", command_id: queued.commandId } });

    cloud.acceptCommand(queued.commandId);
    const [synced] = await syncCommands(phone.context);

    // This replay began during sealing but resumes after the accepted receipt.
    continueSync.release();
    const [stale] = await delayedSync;
    expect(queued.state).toBe("pending_machine");
    expect(commandStatusLabel(queued.state)).toBe("Waiting for the machine");
    expect(commandStatusLabel(queued.state, "soundwave")).toBe("Waiting for soundwave");
    expect(synced.state).toBe("accepted");
    expect(commandStatusLabel(synced.state, "soundwave")).toBe("soundwave received it");
    expect(stale.state).toBe("accepted");
    expect(stale.request).toEqual(queued.request);
    expect((await phone.store.commands(cloud.accountId))[0].state).toBe("accepted");
    expect(cloud.log.filter(r => r.method === "POST" && r.path === "/api/operator/commands")).toHaveLength(1);
  });

  it("refuses a session this device was not granted, before anything leaves the device", async () => {
    const { cloud, target } = await setup();
    const reader = await device(cloud, "read only", []);
    await expect(queueOfflineMessage(reader.context, target, "hello")).rejects.toMatchObject({ reason: "scope_not_granted" });
    expect(cloud.commands.size).toBe(0);
    await expect(
      queueOfflineMessage((await device(cloud, "scoped", [{ hub_id: "hub-soundwave", project_id: "proj-cas", session_id: null, operations: ["operator_message"] }])).context,
        { ...target, sessionName: "another name" }, "hello"),
    ).rejects.toMatchObject({ reason: "session_mismatch" });
  });

  it("keeps one command ID and the same sealed bytes across an outage", async () => {
    const { cloud, target, scope } = await setup();
    const phone = await device(cloud, "phone", scope);
    cloud.faults.push({ method: "POST", pathPrefix: "/api/operator/commands", status: 503, code: "temporarily_unavailable", times: 1 });
    const queued = await queueOfflineMessage(phone.context, target, "during outage");
    expect(queued.state).toBe("submitting");
    expect(commandStatusLabel(queued.state)).toBe("Queued on this device");
    const [retried] = await syncCommands(phone.context);
    expect(retried.state).toBe("pending_machine");
    expect(retried.commandId).toBe(queued.commandId);
    expect(retried.request).toEqual(queued.request);
    expect(cloud.commands.size).toBe(1);
  });

  it("cancels only before the machine's reservation", async () => {
    const { cloud, target, scope } = await setup();
    const phone = await device(cloud, "phone", scope);
    const first = await queueOfflineMessage(phone.context, target, "never mind");
    expect((await cancelOfflineMessage(phone.context, first.commandId))?.state).toBe("cancelled");

    const second = await queueOfflineMessage(phone.context, target, "too late to cancel");
    cloud.commands.get(second.commandId)!.status = "reserved";
    const attempt = await cancelOfflineMessage(phone.context, second.commandId);
    expect(attempt).toMatchObject({ state: "reserved", reason: "handoff_in_progress" });
  });
});
